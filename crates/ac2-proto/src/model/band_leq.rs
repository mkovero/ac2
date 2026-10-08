//! 1/3-octave band Leq of an SPL meter: band windows of any length and weighting against
//! per-band limits, judged at the mic as typed or moved from another place (a receiving
//! room) by a band transfer (`docs/design/band-leq.md`).

use serde::{Deserialize, Serialize};

use super::{LeqConfig, LeqWindow, Weighting};
use crate::units::{Db, DbSpl, Hz, MeasId, Seconds, WallNs};

/// Bands integrated: 1/3 octaves 20 Hz … 10 kHz ([`BAND_NOMINAL_HZ`]).
pub const BAND_COUNT: usize = 28;

/// Bands of the low-frequency table of STM 545/2015, 20 … 200 Hz: the first this many of
/// [`BAND_NOMINAL_HZ`].
pub const LF_BAND_COUNT: usize = 11;

/// Index of the band whose nominal centre is `hz`, if one is.
pub fn band_index(hz: f64) -> Option<usize> {
    BAND_NOMINAL_HZ.iter().position(|&n| n == hz)
}

/// Nominal mid-band frequencies, Hz (IEC 61260-1 Annex E), low to high.
pub const BAND_NOMINAL_HZ: [f64; BAND_COUNT] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
    500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0,
    8000.0, 10000.0,
];

/// Which limit set is in force (STM 545/2015: night 22:00–07:00, day 07:00–22:00, local
/// time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandPeriod {
    /// 07:00–22:00.
    Day,
    /// 22:00–07:00.
    Night,
}

/// §13 impulse correction, added to the band levels while it is in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpulseCorrection {
    /// None.
    #[default]
    None,
    /// +5 dB.
    Plus5,
    /// +10 dB.
    Plus10,
}

/// §13 narrowband (tonal) correction, added to the band levels while it is in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TonalCorrection {
    /// None.
    #[default]
    None,
    /// +3 dB.
    Plus3,
    /// +6 dB.
    Plus6,
}

/// The §13 corrections the operator has put in force (ac2 detects neither character).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandCorrection {
    /// Impulse character.
    pub impulse: ImpulseCorrection,
    /// Narrowband character.
    pub tonal: TonalCorrection,
}

impl BandCorrection {
    /// Total correction, dB: a rating level is the energy average of `L + K`.
    pub fn db(&self) -> f64 {
        let i = match self.impulse {
            ImpulseCorrection::None => 0.0,
            ImpulseCorrection::Plus5 => 5.0,
            ImpulseCorrection::Plus10 => 10.0,
        };
        let t = match self.tonal {
            TonalCorrection::None => 0.0,
            TonalCorrection::Plus3 => 3.0,
            TonalCorrection::Plus6 => 6.0,
        };
        i + t
    }
}

/// One band of a FOH → place transfer: the attenuation and how far it can be trusted
/// (`docs/design/band-leq.md`, *The transfer*).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BandTransferBand {
    /// No background measurement: the difference as is.
    Unchecked {
        /// FOH level − level at the place.
        attenuation: Db,
    },
    /// The level at the place was 10 dB or more above its background.
    Clean {
        /// FOH level − level at the place.
        attenuation: Db,
    },
    /// 3 … 10 dB above the background: its energy subtracted first.
    Corrected {
        /// FOH level − (level at the place − background).
        attenuation: Db,
        /// Level at the place − background.
        margin: Db,
    },
    /// Less than 3 dB above the background: the attenuation is only known to be at least
    /// this (FOH level − background), the bound used.
    Unusable {
        /// The bound.
        at_least: Db,
    },
    /// Not measured in this band.
    Missing,
}

/// A transfer from the meter's mic to the place the band limits are for, per band of
/// [`BAND_NOMINAL_HZ`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandTransferSet {
    /// The operator's name of the place the limits are for ([`Self::DEFAULT_PLACE`] unless
    /// named): every display that names the place uses it.
    pub place: String,
    /// When it was computed (or typed, for an estimate).
    pub measured_at: WallNs,
    /// Measured from band levels, or the operator's estimate.
    pub origin: TransferOrigin,
    /// Per band, low to high.
    pub bands: [BandTransferBand; BAND_COUNT],
}

impl BandTransferSet {
    /// The place named when the operator names none.
    pub const DEFAULT_PLACE: &str = "receiving room";
    /// Longest place name, characters.
    pub const MAX_PLACE_CHARS: usize = 40;

    /// Why `place` cannot name a transfer's place, if it cannot.
    pub fn check_place(place: &str) -> Result<(), String> {
        if place.trim().is_empty() || place.trim() != place {
            return Err("the place of a band transfer is a name without surrounding spaces".into());
        }
        if place.chars().count() > Self::MAX_PLACE_CHARS || place.chars().any(char::is_control) {
            return Err(format!(
                "the place of a band transfer is at most {} printable characters",
                Self::MAX_PLACE_CHARS
            ));
        }
        Ok(())
    }
}

/// Where a band transfer's attenuations come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOrigin {
    /// `spl.band_transfer`: FOH band levels and the place's under the same test signal.
    Measured,
    /// Typed by the operator (`meas.update`) where the place cannot be reached: each
    /// band [`BandTransferBand::Unchecked`] with the guessed attenuation. Judging the
    /// place's limits at FOH is then only as good as the guess, and every display says
    /// so.
    Estimated,
}

/// Where a set of band levels for a transfer comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BandLevelSource {
    /// The band Leq of an SPL meter's band log over `[from, until)` (wall time), with
    /// the sensitivity each second was logged with: a steady test signal at FOH, the same
    /// mic moved to the place, the place with the system silent — or a second
    /// meter's mic at the place over the same period.
    Log {
        /// SPL meter whose band log is read (it must have the band meter on).
        meas: crate::units::MeasId,
        /// Start (inclusive).
        from: WallNs,
        /// End (exclusive).
        until: WallNs,
    },
    /// Levels typed or imported (`ac2_traces::band_levels`), dB SPL, one per band of
    /// [`BAND_NOMINAL_HZ`]; `None` where not measured.
    Levels {
        /// Per band, low to high ([`BAND_COUNT`] of them).
        levels: Vec<Option<DbSpl>>,
    },
}

/// One logged second of a band meter as `spl.band_log_get` returns it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandLogSecond {
    /// Wall time of the second's start.
    pub start: WallNs,
    /// Time measured within it (0 … 1 s).
    pub measured: Seconds,
    /// Limit set of its local start.
    pub period: BandPeriod,
    /// §13 correction in force (not included in `levels`).
    pub correction: Db,
    /// dB SPL of 0 dBFS in force, if calibrated.
    pub sensitivity: Option<Db>,
    /// Band Leq per band of [`BAND_NOMINAL_HZ`]: dB SPL when `sensitivity` is set, else
    /// dBFS; `None`: no energy in the band.
    pub levels: [Option<f64>; BAND_COUNT],
}

/// The energy average of a span of a band log: what `spl.band_transfer` takes from a
/// [`BandLevelSource::Log`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandLogAverage {
    /// Seconds logged in the span.
    pub seconds: u32,
    /// Time measured within them.
    pub measured: Seconds,
    /// Of `seconds`, those logged without a sensitivity.
    pub uncalibrated: u32,
    /// Per band, dB SPL, each second on the sensitivity it was logged with, the §13
    /// correction left out; `None` when the span has no seconds, measured nothing or has an
    /// uncalibrated second (a band without energy is `None` in it).
    pub levels: Option<[Option<DbSpl>; BAND_COUNT]>,
}

/// `spl.band_log_get`: a span of an SPL meter's band log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplBandLog {
    /// SPL meter.
    pub meas: crate::units::MeasId,
    /// Start (inclusive).
    pub from: WallNs,
    /// End (exclusive).
    pub until: WallNs,
    /// Every `step`-th logged second is in `rows`; `None`: no rows, the average alone.
    pub step: Option<u32>,
    /// Over every second of the span, whatever `step`.
    pub average: BandLogAverage,
    /// The seconds asked for, oldest first.
    pub rows: Vec<BandLogSecond>,
}

impl SplBandLog {
    /// Most rows one reply carries: an hour of seconds. A span and step that would return
    /// more is refused, naming the step that fits.
    pub const MAX_ROWS: u32 = 3600;
}

/// The predicted A-weighted level at the transfer's place, a rolling window judged against
/// a day and a night limit (§12: music at night ≤ LAeq,1h 25 dB in rooms meant for sleeping;
/// Liite 2 Taulukko 1 in living rooms). Only with a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PredictedWindow {
    /// Length: whole seconds, 1 s … [`LeqWindow::MAX_SECONDS`].
    pub duration: Seconds,
    /// 07:00–22:00.
    pub day: Option<DbSpl>,
    /// 22:00–07:00.
    pub night: Option<DbSpl>,
    /// "Near" within this much below the limit (≥ 0).
    pub warn_margin: Db,
}

/// A band window's limits, dB SPL per band of [`BAND_NOMINAL_HZ`] (`None`: the band has
/// none), on the window's weighted band levels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BandLimitSet {
    /// The same limits day and night.
    Always {
        /// Per band, low to high.
        limits: [Option<DbSpl>; BAND_COUNT],
    },
    /// Night limits (22:00–07:00, local time) and the day's (07:00–22:00) `day_offset`
    /// higher; a window holding any night second is judged by the night limits.
    NightDay {
        /// Per band, low to high.
        night: [Option<DbSpl>; BAND_COUNT],
        /// Day limits are the night's plus this.
        day_offset: Db,
    },
}

impl Default for BandLimitSet {
    fn default() -> Self {
        BandLimitSet::Always {
            limits: [None; BAND_COUNT],
        }
    }
}

impl BandLimitSet {
    /// The limits 22:00–07:00 (all day for [`Self::Always`]).
    pub fn night(&self) -> &[Option<DbSpl>; BAND_COUNT] {
        match self {
            BandLimitSet::Always { limits } => limits,
            BandLimitSet::NightDay { night, .. } => night,
        }
    }

    /// The limits to change: the night's, or the one set.
    pub fn night_mut(&mut self) -> &mut [Option<DbSpl>; BAND_COUNT] {
        match self {
            BandLimitSet::Always { limits } => limits,
            BandLimitSet::NightDay { night, .. } => night,
        }
    }

    /// The day's offset over the night limits; `None` for one set.
    pub fn day_offset(&self) -> Option<Db> {
        match self {
            BandLimitSet::Always { .. } => None,
            BandLimitSet::NightDay { day_offset, .. } => Some(*day_offset),
        }
    }

    /// The limit of band `band` in `period`.
    pub fn of(&self, period: BandPeriod, band: usize) -> Option<DbSpl> {
        let l = self.night().get(band).copied().flatten()?;
        Some(match (self.day_offset(), period) {
            (Some(o), BandPeriod::Day) => DbSpl(l.0 + o.0),
            _ => l,
        })
    }

    /// Whether any band has a limit.
    pub fn any(&self) -> bool {
        self.night().iter().any(Option::is_some)
    }
}

/// One rolling band window: every shown band's Leq over the same length and weighting,
/// each against its own limit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandWindow {
    /// Length: whole seconds, 1 s … [`LeqWindow::MAX_SECONDS`].
    pub duration: Seconds,
    /// Frequency weighting of the band levels: the weighting at each band's exact mid-band
    /// frequency added to its unweighted level (`docs/design/band-leq.md`, *Weighting a
    /// band*).
    pub weighting: Weighting,
    /// Per-band limits, at the transfer's place when there is a transfer, else at the mic.
    pub limits: BandLimitSet,
    /// "Near" within this much below a limit (≥ 0).
    pub warn_margin: Db,
}

impl BandWindow {
    /// A window of `minutes` with `weighting`, no limits, the default warn margin.
    pub fn minutes(minutes: u32, weighting: Weighting) -> Self {
        Self {
            duration: Seconds(f64::from(minutes) * 60.0),
            weighting,
            limits: BandLimitSet::default(),
            warn_margin: Db(LeqWindow::DEFAULT_WARN_MARGIN_DB),
        }
    }

    /// Length in whole seconds, when it is one in range.
    pub fn seconds(&self) -> Option<u32> {
        whole_seconds(self.duration)
    }
}

fn whole_seconds(d: Seconds) -> Option<u32> {
    let s = d.0;
    (s.is_finite() && s >= 1.0 && s <= f64::from(LeqWindow::MAX_SECONDS) && s.fract() == 0.0)
        .then_some(s as u32)
}

/// The band meter of an SPL meter: the 1/3-octave band Leq of the unweighted (mic-curve
/// corrected) input in rolling band windows of their own length and weighting, on the
/// bands the operator keeps, against per-band limits; with a transfer, the limits are moved
/// from its place to the mic and the A-weighted level there is predicted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandLeqConfig {
    /// Windows, in display order (at most [`LeqConfig::MAX_WINDOWS`]).
    pub windows: Vec<BandWindow>,
    /// Nominal centres of the bands shown, judged and alarmed, low to high (at least one,
    /// each of [`BAND_NOMINAL_HZ`]); every band is integrated and logged whatever this.
    pub bands: Vec<Hz>,
    /// The predicted level at the transfer's place; `None`: not predicted.
    pub predicted: Option<PredictedWindow>,
    /// §13 corrections in force (each second is logged with the one in force).
    pub correction: BandCorrection,
    /// Transfer from the mic to the place the limits are for; without one the limits are
    /// judged at the mic as they are.
    pub transfer: Option<BandTransferSet>,
}

/// Informational presets of the band meter. Not legal advice: a prediction from FOH is
/// not a measurement in the receiving room (`docs/design/band-leq.md`, *What is and isn't
/// claimed*). A preset replaces the windows, the bands and the predicted window; the
/// correction and the transfer stay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BandLeqPreset {
    /// STM 545/2015 §12 and Liite 2 Taulukko 2: LZeq 60 min per band 20 … 200 Hz (night 74
    /// … 32 dB, day 5 dB higher), and the predicted LAeq 60 min ≤ 25 dB at night.
    Finland545Lf,
    /// STM 545/2015 Liite 2 Taulukko 1, living rooms: the predicted LAeq 60 min, day 35,
    /// night 30 dB; the bands 20 … 200 Hz shown in an LZeq 60 min window without limits.
    Finland545LivingRoom,
}

/// Where the band limits a band meter judges at its mic come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandLimitPlace {
    /// No transfer: the limits as typed, at the mic.
    AtMic,
    /// The place's limits plus each band's attenuation (the bound for an unusable band; a
    /// missing band has none).
    Transferred,
    /// As [`Self::Transferred`], with the operator's estimated attenuation
    /// ([`TransferOrigin::Estimated`]).
    Estimated,
}

/// The predicted level at the transfer's place, as the `band_leq` frame carries it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PredictedLeq {
    /// Window length.
    pub duration: Seconds,
    /// LAeq of the window from the bands with a measured attenuation, dB SPL (NaN before
    /// anything was measured or uncalibrated).
    pub estimate: f64,
    /// Adding the unusable bands at their bound: the place is at most this.
    pub at_most: f64,
    /// The limit in force.
    pub limit: Option<DbSpl>,
    /// `estimate` judged against it.
    pub judgement: super::LeqJudgement,
}

impl BandLeqConfig {
    /// The shown bands' indices into [`BAND_NOMINAL_HZ`], low to high; `None` when one is
    /// not a band or they are not in order.
    pub fn band_indices(&self) -> Option<Vec<usize>> {
        let idx: Option<Vec<usize>> = self.bands.iter().map(|h| band_index(h.0)).collect();
        idx.filter(|v| v.windows(2).all(|w| w[0] < w[1]))
    }

    /// Why the configuration cannot run, if it cannot.
    pub fn check(&self) -> Result<(), String> {
        if self.windows.len() > LeqConfig::MAX_WINDOWS {
            return Err(format!(
                "at most {} band windows per meter",
                LeqConfig::MAX_WINDOWS
            ));
        }
        if self.bands.is_empty() || self.band_indices().is_none() {
            return Err(
                "the band meter shows one band or more, each a 1/3-octave band 20 Hz … 10 kHz \
                 named once, low to high"
                    .into(),
            );
        }
        let finite = |l: &Option<DbSpl>| l.is_none_or(|l| l.0.is_finite());
        let margin = |m: Db| m.0.is_finite() && m.0 >= 0.0;
        for w in &self.windows {
            if w.seconds().is_none() {
                return Err("a band window is 1 s … 24 h in whole seconds".into());
            }
            if !w.limits.night().iter().all(finite)
                || w.limits.day_offset().is_some_and(|o| !o.0.is_finite())
            {
                return Err("a band limit must be finite".into());
            }
            if !margin(w.warn_margin) {
                return Err("a band window's warn margin is 0 dB or more".into());
            }
        }
        if let Some(p) = &self.predicted {
            if whole_seconds(p.duration).is_none() {
                return Err("the predicted window is 1 s … 24 h in whole seconds".into());
            }
            if !(finite(&p.day) && finite(&p.night)) {
                return Err("a predicted limit must be finite".into());
            }
            if !margin(p.warn_margin) {
                return Err("the predicted window's warn margin is 0 dB or more".into());
            }
        }
        if let Some(t) = &self.transfer {
            BandTransferSet::check_place(&t.place)?;
            let ok = |d: Db| d.0.is_finite();
            if !t.bands.iter().all(|b| match *b {
                BandTransferBand::Unchecked { attenuation }
                | BandTransferBand::Clean { attenuation } => ok(attenuation),
                BandTransferBand::Corrected {
                    attenuation,
                    margin,
                } => ok(attenuation) && ok(margin),
                BandTransferBand::Unusable { at_least } => ok(at_least),
                BandTransferBand::Missing => true,
            }) {
                return Err("a transfer attenuation must be finite".into());
            }
        }
        Ok(())
    }

    /// The predicted window's length in whole seconds, when it has one in range.
    pub fn predicted_seconds(&self) -> Option<u32> {
        self.predicted.and_then(|p| whole_seconds(p.duration))
    }
}

impl BandLeqPreset {
    /// Every preset, in the order the app offers them.
    pub const ALL: [BandLeqPreset; 2] = [
        BandLeqPreset::Finland545Lf,
        BandLeqPreset::Finland545LivingRoom,
    ];

    /// Night limits of STM 545/2015 Liite 2 Taulukko 2, 20 … 200 Hz, dB.
    pub const FINLAND_545_NIGHT_DB: [f64; LF_BAND_COUNT] = [
        74.0, 64.0, 56.0, 49.0, 44.0, 42.0, 40.0, 38.0, 36.0, 34.0, 32.0,
    ];
    /// Day limits are this much higher ("Päiväajan (klo 7–22) pienitaajuiselle melulle
    /// sovelletaan 5 dB suurempia arvoja").
    pub const FINLAND_545_DAY_OFFSET_DB: f64 = 5.0;

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            BandLeqPreset::Finland545Lf => "Finland STM 545/2015, low frequencies",
            BandLeqPreset::Finland545LivingRoom => "Finland STM 545/2015, living room",
        }
    }

    /// The rule's source.
    pub fn source(self) -> &'static str {
        match self {
            BandLeqPreset::Finland545Lf => {
                "Asumisterveysasetus STM 545/2015 §12 and Liite 2 Taulukko 2, rooms meant for \
                 sleeping; music at night LAeq,1h 25 dB"
            }
            BandLeqPreset::Finland545LivingRoom => {
                "Asumisterveysasetus STM 545/2015 Liite 2 Taulukko 1, living rooms"
            }
        }
    }

    /// The bands the preset shows: 20 … 200 Hz.
    pub fn bands(self) -> Vec<Hz> {
        BAND_NOMINAL_HZ[..LF_BAND_COUNT]
            .iter()
            .map(|&h| Hz(h))
            .collect()
    }

    /// The preset's windows.
    pub fn windows(self) -> Vec<BandWindow> {
        let mut w = BandWindow::minutes(60, Weighting::Z);
        if self == BandLeqPreset::Finland545Lf {
            let mut night = [None; BAND_COUNT];
            for (n, l) in night.iter_mut().zip(Self::FINLAND_545_NIGHT_DB) {
                *n = Some(DbSpl(l));
            }
            w.limits = BandLimitSet::NightDay {
                night,
                day_offset: Db(Self::FINLAND_545_DAY_OFFSET_DB),
            };
        }
        vec![w]
    }

    /// The preset's predicted window.
    pub fn predicted(self) -> PredictedWindow {
        let (day, night) = match self {
            BandLeqPreset::Finland545Lf => (None, Some(DbSpl(25.0))),
            BandLeqPreset::Finland545LivingRoom => (Some(DbSpl(35.0)), Some(DbSpl(30.0))),
        };
        PredictedWindow {
            duration: Seconds(3600.0),
            day,
            night,
            warn_margin: Db(LeqWindow::DEFAULT_WARN_MARGIN_DB),
        }
    }

    /// `to` with the preset's windows, bands and predicted window, its correction and
    /// transfer kept (a preset never discards a measured transfer); a band meter turned on
    /// by the preset starts without either.
    pub fn apply(self, to: Option<&BandLeqConfig>) -> BandLeqConfig {
        BandLeqConfig {
            windows: self.windows(),
            bands: self.bands(),
            predicted: Some(self.predicted()),
            correction: to.map(|c| c.correction).unwrap_or_default(),
            transfer: to.and_then(|c| c.transfer.clone()),
        }
    }
}

/// Two spans of a transfer on the same meter that overlap: one mic cannot be at FOH and at
/// the transfer's `place` at once, nor hear the test signal and the silence at once. The
/// refusal names them and the meter (`name` of its id), or `None` when every pair is apart
/// (or on different meters).
pub fn overlapping_spans(
    foh: &BandLevelSource,
    at_place: &BandLevelSource,
    background: Option<&BandLevelSource>,
    place: &str,
    name: impl Fn(MeasId) -> String,
) -> Option<String> {
    let span = |s: &BandLevelSource| match s {
        BandLevelSource::Log { meas, from, until } => Some((*meas, *from, *until)),
        BandLevelSource::Levels { .. } => None,
    };
    let named = [
        ("FOH", Some(foh)),
        (place, Some(at_place)),
        ("background", background),
    ];
    for (i, (a_name, a)) in named.iter().enumerate() {
        for (b_name, b) in &named[i + 1..] {
            let (Some((ma, fa, ua)), Some((mb, fb, ub))) = (a.and_then(span), b.and_then(span))
            else {
                continue;
            };
            if ma == mb && fa < ub && fb < ua {
                return Some(format!(
                    "the {a_name} and {b_name} spans overlap on {}: one mic cannot \
                     be in two places at once; mark them one after another (the same \
                     test-signal level for FOH and {place}), or measure the {place} with a \
                     second meter",
                    name(ma)
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod overlap_tests {
    use super::*;

    #[test]
    fn spans_of_one_meter_must_not_overlap() {
        let log = |meas: u32, from: u64, until: u64| BandLevelSource::Log {
            meas: MeasId(meas),
            from: WallNs(from * 1_000_000_000),
            until: WallNs(until * 1_000_000_000),
        };
        let n = |m: MeasId| format!("meter {}", m.0);
        let typed = BandLevelSource::Levels {
            levels: vec![None; BAND_COUNT],
        };
        // One mic moved: FOH, then the place, then the background.
        assert_eq!(
            overlapping_spans(
                &log(1, 0, 60),
                &log(1, 120, 180),
                Some(&log(1, 200, 260)),
                "flat 4",
                n
            ),
            None
        );
        // Two mics: the same span on two meters.
        assert_eq!(
            overlapping_spans(&log(1, 0, 60), &log(2, 0, 60), None, "flat 4", n),
            None
        );
        assert_eq!(
            overlapping_spans(&typed, &log(1, 0, 60), None, "flat 4", n),
            None
        );
        let e = overlapping_spans(&log(1, 0, 60), &log(1, 30, 90), None, "flat 4", n)
            .unwrap_or_default();
        assert!(
            e.starts_with("the FOH and flat 4 spans overlap on meter 1"),
            "{e}"
        );
        let e = overlapping_spans(
            &log(1, 0, 60),
            &log(2, 0, 60),
            Some(&log(2, 59, 90)),
            "flat 4",
            n,
        )
        .unwrap_or_default();
        assert!(
            e.starts_with("the flat 4 and background spans overlap"),
            "{e}"
        );
    }
}
