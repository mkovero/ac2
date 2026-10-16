//! `ac2 meas …` and the measurement value types shared with other commands.

use std::str::FromStr;

use clap::{Args, Subcommand, ValueEnum};

use super::{ColourArg, ResolutionArg};
use crate::units::{Channel, Channels, Freq, MeasLevel, SampleCount, SpecAveragingArg, Time};

/// Kind of a new measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MeasKindArg {
    /// Dual-channel transfer function.
    Tf,
    /// Narrowband spectrum.
    Spectrum,
    /// Fractional-octave RTA.
    Rta,
    /// SPL meter.
    Spl,
    /// Sweep measurement: settings only (`--ref`, `--meas`, `--out`, `--level`, …); `ac2
    /// sweep run` plays it.
    Sweep,
}

/// Sweep harmonic windows at the lowest columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum LfHarmonicsArg {
    /// Every order in one shared window: the lowest floor.
    #[default]
    Standard,
    /// Each order in its own longest window: finer low-frequency harmonics, a higher floor
    /// there, a longer silence after the sweep.
    Fine,
}

impl From<LfHarmonicsArg> for ac2_proto::model::LfHarmonics {
    fn from(a: LfHarmonicsArg) -> Self {
        match a {
            LfHarmonicsArg::Standard => Self::Standard,
            LfHarmonicsArg::Fine => Self::Fine,
        }
    }
}

/// Frequency weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WeightArg {
    /// A.
    A,
    /// C.
    C,
    /// Z (flat).
    Z,
}

/// SLM time weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TimeWeightArg {
    /// 125 ms.
    Fast,
    /// 1 s.
    Slow,
    /// Impulse.
    Impulse,
}

/// FFT window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WindowArg {
    /// Hann.
    Hann,
    /// 4-term Blackman-Harris.
    Bh4,
    /// Flat-top.
    Flattop,
    /// Rectangular.
    Rect,
}

/// What a spectrum's levels are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SpecLevelArg {
    /// Tone level (dBFS): a sine reads its level.
    Tone,
    /// Power spectral density (dBFS/Hz): noise reads the same at any resolution.
    Density,
}

/// Octave fraction 1/N.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FractionArg {
    /// 1/1.
    #[value(name = "1")]
    F1,
    /// 1/3.
    #[value(name = "3")]
    F3,
    /// 1/6.
    #[value(name = "6")]
    F6,
    /// 1/12.
    #[value(name = "12")]
    F12,
    /// 1/24.
    #[value(name = "24")]
    F24,
    /// 1/48 (smoothing only).
    #[value(name = "48")]
    F48,
}

/// A measurement (or trace) by id or name, as typed. Which one it means is decided against
/// the daemon's state: a name may be all digits, so `1083` can be an id, a name, or both
/// (then it is refused as ambiguous).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasRef(pub String);

impl MeasRef {
    /// The id it would be, if it is a number.
    pub fn id(&self) -> Option<u32> {
        self.0.parse().ok()
    }
}

impl FromStr for MeasRef {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty measurement reference".into());
        }
        Ok(Self(s.to_owned()))
    }
}

/// `meas …`.
#[derive(Debug, Subcommand)]
pub enum MeasCmd {
    /// Create a measurement.
    New(MeasNew),
    /// List measurements.
    List {
        /// Live view with data age and STALE.
        #[arg(long)]
        watch: bool,
    },
    /// Change a measurement's averaging (`--average`: spectrum, rta; `--blocks`: tf), FFT
    /// length and window (`--fft`, `--window`: spectrum), Resolution (`--resolution`: tf,
    /// sweep; `multi[:1/N]` for a spectrum's multi-resolution bands, `--fft` back to one
    /// FFT) or colour (`--colour`). What is not given stays as it is; a running
    /// measurement starts its average afresh (a colour alone leaves it running as it is),
    /// a sweep's Resolution applies to its next run.
    Set {
        /// Id or name.
        meas: MeasRef,
        /// Spectrum and RTA averaging, on power: `off`, `fifo:<frames>` (the mean of the last
        /// frames: a spectrum's hop, an RTA's result interval of about 1/60 s, each weighted
        /// by its length), `exp:<time>` (exponential, e.g. `exp:2s`).
        #[arg(long, value_name = "AVG")]
        average: Option<SpecAveragingArg>,
        /// tf averaging: FIFO blocks of the full-rate stage (≥ 1).
        #[arg(long)]
        blocks: Option<u32>,
        /// Spectrum FFT length: a power of two, 64 … 65536 samples. A math channel's
        /// operand keeps its length (edit the math channel first).
        #[arg(long, value_name = "SAMPLES")]
        fft: Option<SampleCount>,
        /// Spectrum window.
        #[arg(long, value_enum)]
        window: Option<WindowArg>,
        /// Spectrum level: `tone` (a sine reads its level) or `density` (per hertz: noise
        /// reads alike at any resolution). Changing between one FFT and `multi` without it
        /// takes the new resolution's default (tone for one FFT, density for multi).
        #[arg(long, value_enum)]
        level: Option<SpecLevelArg>,
        /// tf and sweep: Resolution, points per octave stored: `1/12`, `1/24`, `1/48`,
        /// `1/96` (not smoothing). Spectrum: `multi` or `multi:1/N`, the multi-resolution
        /// bands (long windows low, short high) on a 1/N-octave grid.
        #[arg(long, value_name = "1/N|multi[:1/N]")]
        resolution: Option<ResolutionArg>,
        /// The colour its curves are drawn in: `1` … `8` (the app's `colour N`), `grey`,
        /// or `auto` (by its id, the default). Not for a math channel (its measurement's).
        #[arg(long, value_name = "N|grey|auto")]
        colour: Option<ColourArg>,
    },
    /// Make an SPL meter the rig's (kept by the rig across session loads, new sessions and
    /// restarts, and metering whatever session is open) or the session's again.
    Scope {
        /// Id or name.
        meas: MeasRef,
        /// `rig` or `session`.
        #[arg(value_enum)]
        scope: super::ScopeArg,
    },
    /// Start a measurement.
    Start {
        /// Id or name.
        meas: MeasRef,
    },
    /// Stop a measurement.
    Stop {
        /// Id or name.
        meas: MeasRef,
    },
    /// A measurement's own panes: the app shows them while it is selected (edited in the
    /// app).
    Layout {
        #[command(subcommand)]
        cmd: MeasLayoutCmd,
    },
    /// Delete a measurement. One that owns stored traces or math channels needs
    /// `--keep-traces` (they move to the imported group) or `--delete-traces`.
    Rm {
        /// Id or name.
        meas: MeasRef,
        /// Keep its traces and math channels, moved to the imported group.
        #[arg(long, conflicts_with = "delete_traces")]
        keep_traces: bool,
        /// Delete its traces and math channels with it.
        #[arg(long)]
        delete_traces: bool,
    },
}

/// `meas layout …`.
#[derive(Debug, Subcommand)]
pub enum MeasLayoutCmd {
    /// Clear a measurement's own panes: selecting it keeps the panes on screen.
    Clear {
        /// Id or name.
        meas: MeasRef,
    },
}

/// `meas new`.
#[derive(Debug, Args)]
pub struct MeasNew {
    /// Kind.
    #[arg(value_enum)]
    pub kind: MeasKindArg,
    /// Name.
    #[arg(long)]
    pub name: String,
    /// Reference input (tf, sweep).
    #[arg(long = "ref")]
    pub reference: Option<Channel>,
    /// Measurement input (tf, sweep).
    #[arg(long = "meas")]
    pub measurement: Option<Channel>,
    /// Sweep: outputs playing it, the speaker's and the loopback's, e.g. `1,2`.
    #[arg(long = "out", value_name = "OUTS")]
    pub outputs: Option<Channels>,
    /// Sweep: RMS level, e.g. `-50dbfs`; required for a sweep (there is no default level).
    /// Spectrum: `tone` (a sine reads its level) or `density` (per hertz: noise reads alike
    /// at any resolution); default tone for one FFT, density for `--resolution multi`.
    #[arg(long, allow_hyphen_values = true)]
    pub level: Option<MeasLevel>,
    /// Sweep: duration (default 3 s).
    #[arg(long, value_name = "TIME")]
    pub duration: Option<Time>,
    /// Sweep: sweeps played and averaged per run (1 … 8, default 1).
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=8))]
    pub repeats: Option<u8>,
    /// Sweep: gate the linear response this long after the arrival.
    #[arg(long, value_name = "TIME")]
    pub gate: Option<Time>,
    /// Sweep: silence recorded after each sweep (at most 20 s).
    #[arg(long, value_name = "TIME")]
    pub tail: Option<Time>,
    /// Sweep: LF harmonics (default standard); fine: finer low-frequency harmonics, a
    /// higher floor there, a longer silence after the sweep.
    #[arg(long, value_enum)]
    pub lf_harmonics: Option<LfHarmonicsArg>,
    /// Input (spectrum, rta, spl).
    #[arg(long)]
    pub input: Option<Channel>,
    /// tf and sweep: Resolution, the points per octave stored, `1/12`, `1/24`, `1/48`
    /// (default) or `1/96`. Not smoothing (a display edit, `--smooth`): a finer resolution
    /// adds columns, and below a frequency set by the analysis window they are
    /// interpolation, not detail (the plot marks where). Spectrum: `multi` or `multi:1/N`
    /// instead of one `--fft`: six windows from 0.68 s at the lowest frequencies to 21 ms
    /// at the highest, on 1/N-octave columns (1/48 by default).
    #[arg(long, value_name = "1/N|multi[:1/N]")]
    pub resolution: Option<ResolutionArg>,
    /// Display smoothing, 1/N octave (tf: magnitude and phase; spectrum: power, and the
    /// level then no longer reads as tone level).
    #[arg(long, value_enum)]
    pub smooth: Option<FractionArg>,
    /// tf: smooth the magnitude only and keep the measured phase.
    #[arg(long, requires = "smooth")]
    pub smooth_magnitude_only: bool,
    /// tf averaging: FIFO blocks of the full-rate stage.
    #[arg(long, default_value_t = ac2_proto::model::TransferConfig::DEFAULT_BLOCKS)]
    pub blocks: u32,
    /// tf: cap the low-frequency stages' averaging span (default 1s) for faster settling,
    /// at a higher coherence floor there. Default: equal confidence at every frequency.
    #[arg(long, num_args = 0..=1, default_missing_value = "1s", value_name = "TIME")]
    pub fast_lf: Option<Time>,
    /// Spectrum and RTA averaging, on power (default off): `off`, `fifo:<frames>` (the mean
    /// of the last frames: a spectrum's hop, an RTA's result interval of about 1/60 s, each
    /// weighted by its length), `exp:<time>` (exponential, e.g. `exp:2s`).
    #[arg(long, value_name = "AVG")]
    pub average: Option<SpecAveragingArg>,
    /// Spectrum FFT length: a power of two, 64 … 65536 samples (not with
    /// `--resolution multi`).
    #[arg(long, default_value = "65536samples")]
    pub fft: SampleCount,
    /// Spectrum window.
    #[arg(long, value_enum, default_value = "hann")]
    pub window: WindowArg,
    /// RTA band fraction, 1/N octave.
    #[arg(long, value_enum, default_value = "3")]
    pub fraction: FractionArg,
    /// RTA lowest band; sweep start frequency.
    #[arg(long, default_value = "20hz")]
    pub from: Freq,
    /// RTA highest band; sweep end frequency.
    #[arg(long, default_value = "20khz")]
    pub to: Freq,
    /// Frequency weighting (rta: default z; spl: default a).
    #[arg(long, value_enum)]
    pub weight: Option<WeightArg>,
    /// Time weighting (spl).
    #[arg(long, value_enum, default_value = "fast")]
    pub time: TimeWeightArg,
    /// Start right away.
    #[arg(long)]
    pub start: bool,
}
