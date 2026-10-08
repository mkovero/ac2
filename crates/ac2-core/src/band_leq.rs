//! Rolling 1/3-octave band Leq with per-band limits, a day/night limit set and a
//! FOH → dwelling transfer (`docs/design/band-leq.md`).
//!
//! [`BandIntegrator`] filters the unweighted signal through the IEC 61260-1 1/3-octave bank
//! of [`crate::rta`] and sums each band's energy into one-second [`BandSecond`]s, a capture
//! gap adding neither energy nor measured time. [`BandWindows`] keeps one rolling window per
//! limited band on [`RollingLeq`] (the same sums, headroom and recovery as the A/C/Z
//! windows) and judges each band with [`judge_window`] against the limit set in force
//! ([`Period`]). [`Transfer`] turns a setup measurement (band levels at FOH and in the
//! dwelling, with the dwelling's background) into a per-band attenuation, FOH limits and a
//! predicted dwelling LAeq per second ([`PredictedSecond`]).
//!
//! Levels are dBFS on the scale of decision 4a (`10·lg(2·ms)`), as in [`crate::leq`]; limits
//! in dB SPL are given with the sensitivity as an offset, as to [`judge_window`]. Nothing
//! allocates after construction (the bank's decimation buffers grow to the largest block
//! once).

use crate::leq::{
    Headroom, Judgement, RollingLeq, Slot, Verdict, WindowValue, judge_window, mean_square,
};
use crate::rta::{BandFraction, BandInfo, OctaveFilterBank, RtaError};
use crate::spectrum::power_dbfs;
use crate::weighting::Weighting;

/// Bands integrated: 1/3 octaves from 20 Hz to 10 kHz (nominal).
pub const BANDS: usize = 28;

/// The bands with low-frequency limits, 20 … 200 Hz (STM 545/2015 Liite 2 Taulukko 2): the
/// first this many of [`NOMINAL_HZ`].
pub const LF_BANDS: usize = 11;

/// Nominal mid-band frequencies of the bands, Hz (IEC 61260-1 Annex E).
pub const NOMINAL_HZ: [f64; BANDS] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
    500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0,
    8000.0, 10000.0,
];

/// Index of the 1 kHz band in [`NOMINAL_HZ`].
const KHZ_BAND: f64 = 17.0;

/// Exact mid-band frequency of band `band`, Hz: `1000 · 10^(x/10)`, base-ten (IEC 61260-1
/// 5.2), the frequency the filter bank is centred on.
pub fn centre_hz(band: usize) -> f64 {
    1000.0 * 10f64.powf((band as f64 - KHZ_BAND) / 10.0)
}

/// One second of energy per band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandSecond {
    /// Σ y_b² / fs per band of [`NOMINAL_HZ`] (FS²·s).
    pub energy: [f64; BANDS],
    /// Time actually measured within the second (s); 0 for a second lost to a gap.
    pub measured: f64,
}

impl BandSecond {
    /// A second lost to a capture gap.
    pub const GAP: Self = Self {
        energy: [0.0; BANDS],
        measured: 0.0,
    };

    /// From per-band levels over `measured` seconds (a log row read back); a non-finite
    /// level is no energy.
    pub fn from_levels(levels_dbfs: &[f64; BANDS], measured: f64) -> Self {
        let m = measured.clamp(0.0, 1.0);
        let mut energy = [0.0; BANDS];
        for (e, &l) in energy.iter_mut().zip(levels_dbfs) {
            let ms = mean_square(l);
            *e = if ms.is_finite() { ms * m } else { 0.0 };
        }
        Self {
            energy,
            measured: m,
        }
    }

    /// Leq of band `band` over the measured part, dBFS; NaN when nothing was measured.
    pub fn level_dbfs(&self, band: usize) -> f64 {
        if self.measured > 0.0 {
            power_dbfs(self.energy[band] / self.measured)
        } else {
            f64::NAN
        }
    }

    /// With a correction of `db` added to every band level (STM 545/2015 §13: impulse or
    /// narrowband character, added for the time it occurs). A rating level is the energy
    /// average of `L + K` over the period, so the correction scales the second's energy.
    pub fn corrected(self, db: f64) -> Self {
        let g = 10f64.powf(db / 10.0);
        let mut energy = self.energy;
        energy.iter_mut().for_each(|e| *e *= g);
        Self { energy, ..self }
    }
}

impl Slot for BandSecond {
    const GAP: Self = BandSecond::GAP;

    #[inline]
    fn slot_energy(&self, ch: usize) -> f64 {
        self.energy[ch]
    }

    #[inline]
    fn slot_measured(&self) -> f64 {
        self.measured
    }
}

/// Filters the unweighted (mic-curve-corrected) signal through the 1/3-octave bank and sums
/// each band's energy into [`BandSecond`]s on a grid of whole seconds from the first sample.
#[derive(Debug, Clone)]
pub struct BandIntegrator {
    bank: OctaveFilterBank,
    fs: f64,
    per_second: u64,
    /// Samples of the current second passed (measured or skipped).
    pos: u64,
    measured: u64,
    acc: [f64; BANDS],
}

impl BandIntegrator {
    /// At `fs` Hz (a whole number of samples per second). Every band of [`NOMINAL_HZ`] must
    /// lie below Nyquist (the 10 kHz band's upper edge is 11.2 kHz).
    pub fn new(fs: f64) -> Result<Self, RtaError> {
        let lo = centre_hz(0) * 0.99;
        let hi = centre_hz(BANDS - 1) * 1.01;
        let bank = OctaveFilterBank::new(BandFraction::Third, fs, lo, hi)?;
        if bank.len() != BANDS {
            return Err(RtaError::InvalidRate(fs));
        }
        Ok(Self {
            bank,
            fs,
            per_second: (fs.round() as u64).max(1),
            pos: 0,
            measured: 0,
            acc: [0.0; BANDS],
        })
    }

    /// The bands, low to high (as [`NOMINAL_HZ`]).
    pub fn bands(&self) -> impl Iterator<Item = BandInfo> + '_ {
        self.bank.bands()
    }

    /// Samples into the current second.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Filters `block` (measured samples, contiguous with the previous ones); completed
    /// seconds go to `emit`.
    pub fn process(&mut self, mut block: &[f64], mut emit: impl FnMut(BandSecond)) {
        while !block.is_empty() {
            let n = (self.per_second - self.pos).min(block.len() as u64) as usize;
            self.bank.process(&block[..n]);
            block = &block[n..];
            self.measured += n as u64;
            self.pos += n as u64;
            if self.pos == self.per_second {
                self.fold();
                self.flush(&mut emit);
            }
        }
    }

    /// `samples` were lost (a capture discontinuity): the second grid moves on without
    /// energy or measured time; completed seconds go to `emit`. The filters start again
    /// from rest, as their state belongs to samples no longer adjacent to the next ones.
    pub fn skip(&mut self, samples: u64, mut emit: impl FnMut(BandSecond)) {
        if samples == 0 {
            return;
        }
        self.fold();
        self.bank.reset();
        let mut left = samples;
        while left > 0 {
            let room = self.per_second - self.pos;
            if left < room {
                self.pos += left;
                return;
            }
            left -= room;
            self.pos = self.per_second;
            self.flush(&mut emit);
        }
    }

    /// Adds the bank's energy since its last read to the second's.
    fn fold(&mut self) {
        let n = self.bank.samples();
        if n == 0 {
            return;
        }
        let mut p = [0.0; BANDS];
        self.bank.band_powers(&mut p);
        let t = n as f64 / self.fs;
        for (a, ms) in self.acc.iter_mut().zip(p) {
            if ms.is_finite() {
                *a += ms * t;
            }
        }
        self.bank.reset_powers();
    }

    fn flush(&mut self, emit: &mut impl FnMut(BandSecond)) {
        emit(BandSecond {
            energy: self.acc,
            measured: self.measured as f64 / self.per_second as f64,
        });
        self.acc = [0.0; BANDS];
        self.measured = 0;
        self.pos = 0;
    }
}

/// Which limit set is in force: STM 545/2015 §12 and Liite 2 set night limits for
/// 22:00–07:00 and day limits (5 dB higher for the low-frequency bands) for 07:00–22:00,
/// local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Period {
    /// 07:00–22:00.
    Day,
    /// 22:00–07:00.
    Night,
}

impl Period {
    /// Night starts, s after local midnight.
    pub const NIGHT_FROM_S: u32 = 22 * 3600;
    /// Night ends, s after local midnight.
    pub const NIGHT_UNTIL_S: u32 = 7 * 3600;

    /// The period of a second starting `local_seconds_of_day` after local midnight (wall
    /// clock; a leap second, 86 400, is the night's).
    pub fn at(local_seconds_of_day: u32) -> Self {
        let s = local_seconds_of_day;
        if !(Self::NIGHT_UNTIL_S..Self::NIGHT_FROM_S).contains(&s) {
            Period::Night
        } else {
            Period::Day
        }
    }
}

/// Per-band limits of each period, dB SPL (`None`: the band has no limit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLimits {
    /// 07:00–22:00.
    pub day: [Option<f64>; BANDS],
    /// 22:00–07:00.
    pub night: [Option<f64>; BANDS],
}

impl BandLimits {
    /// Night limits as given; day limits `day_offset_db` higher.
    pub fn night_and_offset_day(night: [Option<f64>; BANDS], day_offset_db: f64) -> Self {
        Self {
            day: night.map(|l| l.map(|l| l + day_offset_db)),
            night,
        }
    }

    /// The limits in force in `period`.
    pub fn of(&self, period: Period) -> &[Option<f64>; BANDS] {
        match period {
            Period::Day => &self.day,
            Period::Night => &self.night,
        }
    }
}

/// One band's window judged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandState {
    /// The window.
    pub value: WindowValue,
    /// Leq of the window with the offset, dB (NaN when nothing was measured).
    pub level_db: f64,
    /// The limit judged against (the period in force for the window), dB.
    pub limit_db: Option<f64>,
    /// Judgement against it ([`judge_window`]); `None` without a limit or a value.
    pub verdict: Option<Verdict>,
    /// Headroom over the horizon against the limit in force once the horizon has passed.
    pub headroom: Option<Headroom>,
}

impl BandState {
    /// Severity: ok, near, on course, over.
    fn rank(&self) -> Option<u8> {
        self.verdict.map(|v| match (v.judgement, v.on_course) {
            (Judgement::Ok, _) => 0,
            (Judgement::Near, false) => 1,
            (Judgement::Near, true) => 2,
            (Judgement::Over, _) => 3,
        })
    }
}

/// The band that is worst off: the most severe judgement, then the furthest above (or
/// least below) its limit. `None` when no band has a judgement.
pub fn worst_band(states: &[BandState]) -> Option<usize> {
    states
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let r = s.rank()?;
            let excess = s.limit_db.map_or(f64::NEG_INFINITY, |l| s.level_db - l);
            Some((i, r, excess))
        })
        .max_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)))
        .map(|(i, _, _)| i)
}

/// Rolling windows of one length on the first `bands` bands, with the period each second
/// was in.
#[derive(Debug, Clone)]
pub struct BandWindows {
    ring: RollingLeq<BandSecond>,
    /// Seconds pushed when the newest night second was pushed (1-based: the count after it).
    last_night: Option<u64>,
}

impl BandWindows {
    /// Windows of `seconds` (≥ 1) on bands `0..bands` (at most [`BANDS`]), headroom over
    /// `horizon` s.
    pub fn new(seconds: u32, bands: usize, horizon: u32) -> Self {
        let bands = bands.min(BANDS);
        Self {
            ring: RollingLeq::of_channels((0..bands).map(|b| (seconds, b)), horizon),
            last_night: None,
        }
    }

    /// Adds the newest second (a gap is [`BandSecond::GAP`]) and the period its start was
    /// in, local wall time.
    pub fn push(&mut self, s: BandSecond, period: Period) {
        self.ring.push(s);
        if period == Period::Night {
            self.last_night = Some(self.ring.pushed());
        }
    }

    /// Empties the windows.
    pub fn clear(&mut self) {
        self.ring.clear();
        self.last_night = None;
    }

    /// Empties the windows and pushes `seconds` (oldest first, as [`Placed::seconds`]
    /// gives a log's rows placed by wall time).
    pub fn refill(&mut self, seconds: impl IntoIterator<Item = (BandSecond, Period)>) {
        self.clear();
        for (s, p) in seconds {
            self.push(s, p);
        }
    }

    /// The windows: values, headroom and recovery per band.
    pub fn windows(&self) -> &RollingLeq<BandSecond> {
        &self.ring
    }

    /// Night when a second of the window `ahead` seconds from now, the newest second
    /// included, is a night second.
    fn night_within(&self, ahead: u64) -> bool {
        self.last_night
            .is_some_and(|n| self.ring.pushed() + ahead - n < u64::from(self.ring.capacity()))
    }

    /// The limit set the windows are judged by: night when any second they hold was a
    /// night second, else day.
    pub fn period(&self) -> Period {
        if self.night_within(0) {
            Period::Night
        } else {
            Period::Day
        }
    }

    /// The limit set the windows are judged by once the horizon has passed, given the
    /// period of the second the horizon ends in (`at_horizon`, from the local time then):
    /// the night is longer than any horizon, so the window then holds a night second
    /// exactly when it holds one now that stays in it, or the horizon ends in the night.
    pub fn period_after_horizon(&self, at_horizon: Period) -> Period {
        if at_horizon == Period::Night || self.night_within(u64::from(self.ring.horizon())) {
            Period::Night
        } else {
            Period::Day
        }
    }

    /// Judges every window against `limits` (dB SPL) with the levels raised by `offset_db`
    /// (the sensitivity) and a warn `margin_db`: the verdict against the set in force for
    /// the window now, the headroom against the set in force after the horizon (`at_horizon`
    /// as [`Self::period_after_horizon`]). `out` holds one state per window.
    ///
    /// # Panics
    /// If `out.len()` is not the number of windows.
    pub fn judge(
        &self,
        limits: &BandLimits,
        offset_db: f64,
        margin_db: f64,
        at_horizon: Period,
        out: &mut [BandState],
    ) {
        assert_eq!(out.len(), self.ring.len(), "one state per window");
        let now = limits.of(self.period());
        let ahead = limits.of(self.period_after_horizon(at_horizon));
        for (i, o) in out.iter_mut().enumerate() {
            let value = self.ring.value(i);
            *o = BandState {
                value,
                level_db: value.leq_dbfs + offset_db,
                limit_db: now[i],
                verdict: now[i].and_then(|l| judge_window(&value, offset_db, l, margin_db)),
                headroom: ahead[i].map(|l| self.ring.headroom(i, mean_square(l - offset_db))),
            };
        }
    }
}

/// Logged seconds placed by wall time on the grid of the `seconds` seconds before a
/// moment, as [`RollingLeq::refill`] places A/C/Z seconds: what rebuilds band windows (and
/// anything fed the same seconds, such as the predicted dwelling level) from a log.
#[derive(Debug, Clone)]
pub struct Placed {
    span_start_ns: u64,
    slots: Vec<Option<(BandSecond, Period)>>,
}

impl Placed {
    const NS: u64 = 1_000_000_000;

    /// Places rows `(wall start ns, second, period)`, newest first, in the `seconds` before
    /// `now_ns`; rows older than that end the walk. Two rows in one slot (a clock step) add,
    /// night if either was.
    pub fn new(
        newest_first: impl IntoIterator<Item = (u64, BandSecond, Period)>,
        now_ns: u64,
        seconds: u32,
    ) -> Self {
        let cap = u64::from(seconds.max(1));
        let span_start_ns = now_ns.saturating_sub(cap * Self::NS);
        let mut slots: Vec<Option<(BandSecond, Period)>> = vec![None; cap as usize];
        for (start, s, p) in newest_first {
            let Some(off) = (start + Self::NS / 2).checked_sub(span_start_ns) else {
                break;
            };
            let k = off / Self::NS;
            if k >= cap {
                continue;
            }
            let slot = &mut slots[k as usize];
            *slot = Some(match slot {
                Some((o, op)) => {
                    let mut energy = o.energy;
                    energy.iter_mut().zip(s.energy).for_each(|(a, b)| *a += b);
                    let night = *op == Period::Night || p == Period::Night;
                    (
                        BandSecond {
                            energy,
                            measured: o.measured + s.measured,
                        },
                        if night { Period::Night } else { Period::Day },
                    )
                }
                None => (s, p),
            });
        }
        Self {
            span_start_ns,
            slots,
        }
    }

    /// The seconds from the oldest placed row to the newest slot, oldest first: a slot
    /// without a row is a gap ([`BandSecond::GAP`]) in the period `period_at(its wall
    /// start)`, so a stopped meter's night seconds still put a window under the night
    /// limits. Empty when no row was placed.
    pub fn seconds<'a>(
        &'a self,
        period_at: impl Fn(u64) -> Period + 'a,
    ) -> impl Iterator<Item = (BandSecond, Period)> + 'a {
        let first = self
            .slots
            .iter()
            .position(Option::is_some)
            .unwrap_or(self.slots.len());
        self.slots
            .iter()
            .enumerate()
            .skip(first)
            .map(move |(k, s)| {
                s.unwrap_or_else(|| {
                    (
                        BandSecond::GAP,
                        period_at(self.span_start_ns + k as u64 * Self::NS),
                    )
                })
            })
    }
}

/// The dwelling level must be this far above the dwelling's background for the band's
/// difference to be taken as is: the background then adds at most 0.41 dB.
pub const CLEAN_MARGIN_DB: f64 = 10.0;

/// Below this margin over the background (the background then makes up half the energy or
/// more) the transmitted level cannot be told from the background: the band only bounds
/// the attenuation.
pub const MIN_MARGIN_DB: f64 = 3.0;

/// One band of a FOH → dwelling transfer, from band levels of the same test signal measured
/// over the same period at FOH and in the dwelling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BandTransfer {
    /// No background measured: the level difference as is.
    Unchecked {
        /// FOH level − dwelling level, dB.
        attenuation_db: f64,
    },
    /// At least [`CLEAN_MARGIN_DB`] above the background: the level difference as is.
    Clean {
        /// dB.
        attenuation_db: f64,
    },
    /// [`MIN_MARGIN_DB`] … [`CLEAN_MARGIN_DB`] above the background: the background's
    /// energy subtracted from the dwelling level first.
    Corrected {
        /// dB.
        attenuation_db: f64,
        /// Dwelling level − background, dB.
        margin_db: f64,
    },
    /// Less than [`MIN_MARGIN_DB`] above the background: the transmitted part is below the
    /// background, so the attenuation is only known to be more than FOH − background.
    Unusable {
        /// FOH level − background, dB.
        at_least_db: f64,
    },
    /// A level was missing (not finite).
    Missing,
}

impl BandTransfer {
    /// From the band levels (dB on one scale: both mics calibrated, or one mic moved) at
    /// FOH, in the dwelling, and of the dwelling's background without the signal.
    pub fn measure(foh_db: f64, dwelling_db: f64, background_db: Option<f64>) -> Self {
        if !(foh_db.is_finite() && dwelling_db.is_finite()) {
            return BandTransfer::Missing;
        }
        let Some(bg) = background_db else {
            return BandTransfer::Unchecked {
                attenuation_db: foh_db - dwelling_db,
            };
        };
        if !bg.is_finite() {
            return BandTransfer::Missing;
        }
        let margin = dwelling_db - bg;
        if margin >= CLEAN_MARGIN_DB {
            BandTransfer::Clean {
                attenuation_db: foh_db - dwelling_db,
            }
        } else if margin >= MIN_MARGIN_DB {
            let signal = 10.0 * (10f64.powf(dwelling_db / 10.0) - 10f64.powf(bg / 10.0)).log10();
            BandTransfer::Corrected {
                attenuation_db: foh_db - signal,
                margin_db: margin,
            }
        } else {
            BandTransfer::Unusable {
                at_least_db: foh_db - bg,
            }
        }
    }

    /// The attenuation, when the band was measured clear of the background.
    pub fn attenuation_db(&self) -> Option<f64> {
        match *self {
            BandTransfer::Unchecked { attenuation_db }
            | BandTransfer::Clean { attenuation_db }
            | BandTransfer::Corrected { attenuation_db, .. } => Some(attenuation_db),
            BandTransfer::Unusable { .. } | BandTransfer::Missing => None,
        }
    }

    /// The attenuation or, for an unusable band, its lower bound: the figure that never
    /// overstates how much the building takes away.
    pub fn conservative_db(&self) -> Option<f64> {
        match *self {
            BandTransfer::Unusable { at_least_db } => Some(at_least_db),
            _ => self.attenuation_db(),
        }
    }
}

/// One second of the predicted dwelling A-weighted energy: [`Self::ESTIMATE`] from the bands
/// with a measured attenuation, [`Self::AT_MOST`] adding the unusable bands at their bound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PredictedSecond {
    /// Energy per channel (FS²·s on the FOH scale).
    pub energy: [f64; 2],
    /// Measured time, s.
    pub measured: f64,
}

impl PredictedSecond {
    /// Channel of the estimate.
    pub const ESTIMATE: usize = 0;
    /// Channel of the upper bound.
    pub const AT_MOST: usize = 1;
}

impl Slot for PredictedSecond {
    const GAP: Self = Self {
        energy: [0.0; 2],
        measured: 0.0,
    };

    #[inline]
    fn slot_energy(&self, ch: usize) -> f64 {
        self.energy[ch]
    }

    #[inline]
    fn slot_measured(&self) -> f64 {
        self.measured
    }
}

/// FOH → dwelling transfer per band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transfer {
    bands: [BandTransfer; BANDS],
    /// Energy gain FOH → dwelling with A weighting at the band centre, per channel of
    /// [`PredictedSecond`].
    gain: [[f64; 2]; BANDS],
}

impl Transfer {
    /// From per-band results (a stored transfer).
    pub fn from_bands(bands: [BandTransfer; BANDS]) -> Self {
        let mut gain = [[0.0; 2]; BANDS];
        for (i, (g, b)) in gain.iter_mut().zip(&bands).enumerate() {
            let a = Weighting::A.analytic_db(centre_hz(i));
            let lin = |att: f64| 10f64.powf((a - att) / 10.0);
            g[PredictedSecond::ESTIMATE] = b.attenuation_db().map_or(0.0, lin);
            g[PredictedSecond::AT_MOST] = b.conservative_db().map_or(0.0, lin);
        }
        Self { bands, gain }
    }

    /// From band levels at FOH and in the dwelling over the same period of one test
    /// signal, and the dwelling's background without it.
    pub fn measure(
        foh_db: &[f64; BANDS],
        dwelling_db: &[f64; BANDS],
        background_db: Option<&[f64; BANDS]>,
    ) -> Self {
        let mut bands = [BandTransfer::Missing; BANDS];
        for (i, b) in bands.iter_mut().enumerate() {
            *b = BandTransfer::measure(foh_db[i], dwelling_db[i], background_db.map(|bg| bg[i]));
        }
        Self::from_bands(bands)
    }

    /// Per-band results.
    pub fn bands(&self) -> &[BandTransfer; BANDS] {
        &self.bands
    }

    /// FOH limits that keep the dwelling at `dwelling` limits: each plus the band's
    /// conservative attenuation; a band without a transfer has no FOH limit.
    pub fn foh_limits(&self, dwelling: &BandLimits) -> BandLimits {
        let shift = |l: &[Option<f64>; BANDS]| {
            let mut out = [None; BANDS];
            for (i, o) in out.iter_mut().enumerate() {
                *o = l[i]
                    .zip(self.bands[i].conservative_db())
                    .map(|(l, a)| l + a);
            }
            out
        };
        BandLimits {
            day: shift(&dwelling.day),
            night: shift(&dwelling.night),
        }
    }

    /// The dwelling's A-weighted energy predicted from a FOH second: each band less its
    /// attenuation, A-weighted at the band centre, summed.
    pub fn predict(&self, s: &BandSecond) -> PredictedSecond {
        let mut energy = [0.0; 2];
        for (e, g) in s.energy.iter().zip(&self.gain) {
            energy[0] += e * g[0];
            energy[1] += e * g[1];
        }
        PredictedSecond {
            energy,
            measured: s.measured,
        }
    }
}

#[cfg(test)]
mod tests;
