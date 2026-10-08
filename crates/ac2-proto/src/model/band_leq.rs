//! 1/3-octave band Leq of an SPL meter against per-band limits in a dwelling, carried to
//! the meter's mic by a FOH → dwelling transfer (`docs/design/band-leq.md`).

use serde::{Deserialize, Serialize};

use crate::units::{Db, DbSpl, Hz, MeasId, Seconds, WallNs};

/// Bands integrated: 1/3 octaves 20 Hz … 10 kHz ([`BAND_NOMINAL_HZ`]).
pub const BAND_COUNT: usize = 28;

/// Bands with low-frequency limits, 20 … 200 Hz: the first this many of
/// [`BAND_NOMINAL_HZ`].
pub const LF_BAND_COUNT: usize = 11;

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

/// One band of a FOH → dwelling transfer: the attenuation and how far it can be trusted
/// (`docs/design/band-leq.md`, *The transfer*).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BandTransferBand {
    /// No background measurement: the difference as is.
    Unchecked {
        /// FOH level − dwelling level.
        attenuation: Db,
    },
    /// The dwelling level was 10 dB or more above its background.
    Clean {
        /// FOH level − dwelling level.
        attenuation: Db,
    },
    /// 3 … 10 dB above the background: its energy subtracted first.
    Corrected {
        /// FOH level − (dwelling − background).
        attenuation: Db,
        /// Dwelling level − background.
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

/// A FOH → dwelling transfer, per band of [`BAND_NOMINAL_HZ`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandTransferSet {
    /// When it was computed (or typed, for an estimate).
    pub measured_at: WallNs,
    /// Measured from band levels, or the operator's estimate.
    pub origin: TransferOrigin,
    /// Per band, low to high.
    pub bands: [BandTransferBand; BAND_COUNT],
}

/// Where a band transfer's attenuations come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOrigin {
    /// `spl.band_transfer`: FOH and dwelling band levels under the same test signal.
    Measured,
    /// Typed by the operator (`meas.update`) where the dwelling cannot be reached: each
    /// band [`BandTransferBand::Unchecked`] with the guessed attenuation. Judging the
    /// dwelling's limits at FOH is then only as good as the guess, and every display says
    /// so.
    Estimated,
}

/// Where a set of band levels for a transfer comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BandLevelSource {
    /// The band Leq of an SPL meter's band log over `[from, until)` (wall time), with
    /// the sensitivity each second was logged with: a steady test signal at FOH, the same
    /// mic moved into the dwelling, the dwelling with the system silent — or a second
    /// meter's mic in the dwelling over the same period.
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

/// Limits of the predicted dwelling LAeq window (§12: music at night ≤ 25 dB in rooms for
/// sleeping; Liite 2 Taulukko 1 in living rooms).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PredictedLimits {
    /// 07:00–22:00.
    pub day: Option<DbSpl>,
    /// 22:00–07:00.
    pub night: Option<DbSpl>,
}

/// The band meter of an SPL meter: the 1/3-octave band Leq of the unweighted (mic-curve
/// corrected) input, rolling windows on the bands 20 … 200 Hz judged against limits in a
/// dwelling, and the predicted dwelling LAeq.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandLeqConfig {
    /// Window length of every band and of the predicted LAeq: whole seconds, 1 s … 24 h
    /// (the decree's 1 h).
    pub duration: Seconds,
    /// Band limits in the dwelling 07:00–22:00, dB SPL, 20 … 200 Hz (`None`: no limit).
    pub day: [Option<DbSpl>; LF_BAND_COUNT],
    /// Band limits in the dwelling 22:00–07:00.
    pub night: [Option<DbSpl>; LF_BAND_COUNT],
    /// "Near" within this much below a limit (≥ 0).
    pub warn_margin: Db,
    /// Limits of the predicted dwelling LAeq (only with a transfer).
    pub predicted: PredictedLimits,
    /// §13 corrections in force (each second is logged with the one in force).
    pub correction: BandCorrection,
    /// Where the mic is: at FOH the limits are judged through `transfer`, and without one
    /// not at all (a level at FOH says nothing about the dwelling); in the dwelling they are
    /// judged as they are.
    pub mic: BandMicPlace,
    /// FOH → dwelling transfer; nothing is predicted without one.
    pub transfer: Option<BandTransferSet>,
}

/// Where a band meter's mic is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandMicPlace {
    /// At FOH (or anywhere but the dwelling): the limits need a transfer.
    #[default]
    Foh,
    /// In the dwelling (a bedroom monitor): the limits apply at the mic as they are.
    Dwelling,
}

/// Informational presets of the band meter. Not legal advice: a prediction from FOH is
/// not a measurement in the dwelling (`docs/design/band-leq.md`, *What is and isn't
/// claimed*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BandLeqPreset {
    /// STM 545/2015 §12 and Liite 2 Taulukko 2, rooms for sleeping: unweighted Leq,1h per
    /// band 20 … 200 Hz (night 74 … 32 dB, day 5 dB higher), and music at night LAeq,1h ≤
    /// 25 dB.
    Finland545Lf,
    /// STM 545/2015 Liite 2 Taulukko 1, living rooms: LAeq,1h day 35, night 30 dB, as the
    /// predicted window (no band limits: Taulukko 2 is for rooms for sleeping).
    Finland545LivingRoom,
}

/// A judged band window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandLeqBand {
    /// Nominal centre.
    pub nominal: Hz,
    /// Leq of the window at the mic, in the frame's `scale`, §13 correction included; NaN
    /// before anything was measured.
    pub leq: f64,
    /// The limit in force at the mic (the dwelling's plus the attenuation), dB SPL.
    pub limit: Option<f64>,
    /// Judgement against it.
    pub judgement: super::LeqJudgement,
    /// Filling, and on course to end over the limit at the pace so far (with `near`).
    pub on_course: bool,
    /// Steady level allowed over the horizon to stay at the limit in force once the
    /// horizon has passed; `None` without a judged limit or when it cannot recover.
    pub allowed: Option<f64>,
    /// Seconds to recover playing at the limit, when it cannot within the horizon.
    pub recover: Option<Seconds>,
}

/// The predicted dwelling LAeq window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PredictedLeq {
    /// LAeq of the window from the bands with a measured attenuation, dB SPL (NaN before
    /// anything was measured or uncalibrated).
    pub estimate: f64,
    /// Adding the unusable bands at their bound: the dwelling is at most this.
    pub at_most: f64,
    /// The limit in force.
    pub limit: Option<DbSpl>,
    /// `estimate` judged against it.
    pub judgement: super::LeqJudgement,
}

/// Where the band limits a band meter judges at its mic come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandLimitPlace {
    /// The mic is in the dwelling ([`BandMicPlace::Dwelling`]): its limits as they are.
    AtMic,
    /// The mic is at FOH without a transfer: the limits are shown, not judged.
    NoTransfer,
    /// The dwelling's limits plus each band's attenuation (the bound for an unusable band;
    /// a missing band has none).
    Transferred,
    /// As [`Self::Transferred`], with the operator's estimated attenuation
    /// ([`TransferOrigin::Estimated`]).
    Estimated,
}

impl BandLeqConfig {
    /// Longest window, s (one day).
    pub const MAX_SECONDS: u32 = 86_400;
    /// One hour (the decree's Leq,1h).
    pub const HOUR_S: f64 = 3600.0;

    /// Length in whole seconds, when it is one in range.
    pub fn seconds(&self) -> Option<u32> {
        let s = self.duration.0;
        (s.is_finite() && s >= 1.0 && s <= f64::from(Self::MAX_SECONDS) && s.fract() == 0.0)
            .then_some(s as u32)
    }

    /// Why the configuration cannot run, if it cannot.
    pub fn check(&self) -> Result<(), String> {
        if self.seconds().is_none() {
            return Err("the band window is 1 s … 24 h in whole seconds".into());
        }
        let finite = |l: &Option<DbSpl>| l.is_none_or(|l| l.0.is_finite());
        if !(self.day.iter().all(finite)
            && self.night.iter().all(finite)
            && finite(&self.predicted.day)
            && finite(&self.predicted.night))
        {
            return Err("a band limit must be finite".into());
        }
        if !(self.warn_margin.0.is_finite() && self.warn_margin.0 >= 0.0) {
            return Err("the band warn margin is 0 dB or more".into());
        }
        if let Some(t) = &self.transfer {
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
            BandLeqPreset::Finland545Lf => "Finland STM 545/2015, low frequencies (bedroom)",
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

    /// The configuration the preset sets: its limits, a one-hour window, the default warn
    /// margin, no correction; the `transfer` given (a preset never discards a measured
    /// transfer).
    pub fn config(self, transfer: Option<BandTransferSet>) -> BandLeqConfig {
        let (night, day, predicted) = match self {
            BandLeqPreset::Finland545Lf => (
                Self::FINLAND_545_NIGHT_DB.map(|l| Some(DbSpl(l))),
                Self::FINLAND_545_NIGHT_DB
                    .map(|l| Some(DbSpl(l + Self::FINLAND_545_DAY_OFFSET_DB))),
                PredictedLimits {
                    day: None,
                    night: Some(DbSpl(25.0)),
                },
            ),
            BandLeqPreset::Finland545LivingRoom => (
                [None; LF_BAND_COUNT],
                [None; LF_BAND_COUNT],
                PredictedLimits {
                    day: Some(DbSpl(35.0)),
                    night: Some(DbSpl(30.0)),
                },
            ),
        };
        BandLeqConfig {
            duration: Seconds(BandLeqConfig::HOUR_S),
            day,
            night,
            warn_margin: Db(super::LeqWindow::DEFAULT_WARN_MARGIN_DB),
            predicted,
            correction: BandCorrection::default(),
            mic: BandMicPlace::Foh,
            transfer,
        }
    }
}

/// Two spans of a transfer on the same meter that overlap: one mic cannot be at FOH and in
/// the bedroom at once, nor hear the test signal and the silence at once. The refusal
/// names them and the meter (`name` of its id), or `None` when every pair is apart (or on
/// different meters).
pub fn overlapping_spans(
    foh: &BandLevelSource,
    dwelling: &BandLevelSource,
    background: Option<&BandLevelSource>,
    name: impl Fn(MeasId) -> String,
) -> Option<String> {
    let span = |s: &BandLevelSource| match s {
        BandLevelSource::Log { meas, from, until } => Some((*meas, *from, *until)),
        BandLevelSource::Levels { .. } => None,
    };
    let named = [
        ("FOH", Some(foh)),
        ("bedroom", Some(dwelling)),
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
                     test-signal level for FOH and bedroom), or measure the bedroom with a \
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
        // One mic moved: FOH, then the bedroom, then the background.
        assert_eq!(
            overlapping_spans(
                &log(1, 0, 60),
                &log(1, 120, 180),
                Some(&log(1, 200, 260)),
                n
            ),
            None
        );
        // Two mics: the same span on two meters.
        assert_eq!(
            overlapping_spans(&log(1, 0, 60), &log(2, 0, 60), None, n),
            None
        );
        assert_eq!(overlapping_spans(&typed, &log(1, 0, 60), None, n), None);
        let e = overlapping_spans(&log(1, 0, 60), &log(1, 30, 90), None, n).unwrap_or_default();
        assert!(
            e.starts_with("the FOH and bedroom spans overlap on meter 1"),
            "{e}"
        );
        let e = overlapping_spans(&log(1, 0, 60), &log(2, 0, 60), Some(&log(2, 59, 90)), n)
            .unwrap_or_default();
        assert!(
            e.starts_with("the bedroom and background spans overlap"),
            "{e}"
        );
    }
}
