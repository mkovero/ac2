//! `ac2 levels …`: the programme meters, their settings and their holds.

use std::str::FromStr;

use ac2_proto::model::{Ballistic, CorrelationWindow, LOUDNESS_TARGET_RANGE, OVER_THRESHOLD_RANGE};
use ac2_proto::units::{Dbtp, Lufs};
use clap::{Subcommand, ValueEnum};

use ac2_scene::unit_text::{Channel, LevelDbfs, Time, UnitError};

/// `levels …`.
#[derive(Debug, Subcommand)]
pub enum LevelsCmd {
    /// The programme meters' settings, kept by the rig: what the bars read, the correlation
    /// window, the over threshold, the correlation gate and the loudness target. Fields not
    /// given keep theirs; the rig is shown after.
    #[command(after_help = LEVELS_SET_HELP)]
    Set {
        /// What the bars read: `true-peak` (BS.1770, the default), `type1` (IEC 60268-10
        /// Type I), `type2` (Type II), `vu` (on line inputs with a line calibration; the true
        /// peak elsewhere), or a source's loudness, `momentary` (400 ms) or `short-term`
        /// (3 s), on line and digital sources (the true peak elsewhere).
        #[arg(long, value_enum)]
        ballistic: Option<BallisticArg>,
        /// The correlation window: `0.3s` or `1s`.
        #[arg(long)]
        window: Option<CorrelationWindowArg>,
        /// True peak above which an over is counted: `-1dbtp` (−20 … +6 dBTP).
        #[arg(long, allow_hyphen_values = true)]
        over_threshold: Option<OverThresholdArg>,
        /// RMS below which a pair's side is quiet and no correlation is shown: `-60dbfs`
        /// (−120 … 0 dBFS).
        #[arg(long, allow_hyphen_values = true)]
        gate: Option<LevelDbfs>,
        /// The programme level the loudness bars are drawn around: `-23lufs` (−36 … −5
        /// LUFS; −23, R128's, by default).
        #[arg(long, allow_hyphen_values = true)]
        loudness_target: Option<LoudnessTargetArg>,
    },
    /// Clears the max holds, the over counts and the loudness (integrated, range, maxima)
    /// of every source with an input given, or of every input.
    Reset {
        /// A device input (1-based); repeatable. None: every input.
        #[arg(long = "input")]
        inputs: Vec<Channel>,
    },
}

const LEVELS_SET_HELP: &str = "\
Every ballistic runs all the time, so a change reads a settled meter at once. Under a loudness
ballistic a pair reads one bar, its programme loudness; each input's own column is its true peak.
A sustained clip counts as one over: the next counts only after the true peak fell 1 dB under
the threshold.

Examples: ac2 levels set --ballistic type1 --window 0.3s
          ac2 levels set --over-threshold -1dbtp --gate -50dbfs
          ac2 levels set --ballistic short-term --loudness-target -16lufs";

/// A ballistic as typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BallisticArg {
    /// BS.1770 true peak.
    TruePeak,
    /// IEC 60268-10 Type I quasi-peak.
    Type1,
    /// IEC 60268-10 Type II quasi-peak.
    Type2,
    /// IEC 60268-17 VU.
    Vu,
    /// BS.1770 momentary loudness (400 ms) of the bar's source.
    Momentary,
    /// BS.1770 short-term loudness (3 s) of the bar's source.
    ShortTerm,
}

impl From<BallisticArg> for Ballistic {
    fn from(b: BallisticArg) -> Self {
        match b {
            BallisticArg::TruePeak => Self::TruePeak,
            BallisticArg::Type1 => Self::Type1,
            BallisticArg::Type2 => Self::Type2,
            BallisticArg::Vu => Self::Vu,
            BallisticArg::Momentary => Self::Momentary,
            BallisticArg::ShortTerm => Self::ShortTerm,
        }
    }
}

/// A correlation window: one of the two the rig offers, as a time (`0.3s`, `300ms`, `1s`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationWindowArg(pub CorrelationWindow);

impl FromStr for CorrelationWindowArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let t = s.parse::<Time>()?;
        CorrelationWindow::ALL
            .into_iter()
            .find(|w| (f64::from(w.seconds()) - t.0.0).abs() < 1e-6)
            .map(Self)
            .ok_or_else(|| UnitError(format!("{s:?}: the correlation window is 0.3s or 1s")))
    }
}

/// An over threshold in dBTP: `-1dbtp`, within [`OVER_THRESHOLD_RANGE`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverThresholdArg(pub Dbtp);

impl FromStr for OverThresholdArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let t = s.trim().to_ascii_lowercase();
        let Some(n) = t.strip_suffix("dbtp") else {
            return Err(UnitError(format!(
                "{s:?}: expected a true-peak level in dBTP (e.g. -1dbtp)"
            )));
        };
        let v: f64 = n
            .trim()
            .parse()
            .map_err(|_| UnitError(format!("{s:?}: not a number of dBTP")))?;
        let (lo, hi) = OVER_THRESHOLD_RANGE;
        if !v.is_finite() || v < lo || v > hi {
            return Err(UnitError(format!(
                "{s:?}: the over threshold is {lo} … +{hi} dBTP"
            )));
        }
        Ok(Self(Dbtp(v)))
    }
}

/// A loudness target in LUFS: `-23lufs`, within [`LOUDNESS_TARGET_RANGE`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessTargetArg(pub Lufs);

impl FromStr for LoudnessTargetArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let t = s.trim().to_ascii_lowercase();
        let Some(n) = t.strip_suffix("lufs") else {
            return Err(UnitError(format!(
                "{s:?}: expected a loudness in LUFS (e.g. -23lufs)"
            )));
        };
        let v: f64 = n
            .trim()
            .parse()
            .map_err(|_| UnitError(format!("{s:?}: not a number of LUFS")))?;
        let (lo, hi) = LOUDNESS_TARGET_RANGE;
        if !v.is_finite() || v < lo || v > hi {
            return Err(UnitError(format!(
                "{s:?}: the loudness target is {lo} … {hi} LUFS"
            )));
        }
        Ok(Self(Lufs(v)))
    }
}
