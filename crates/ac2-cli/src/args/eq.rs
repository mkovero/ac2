//! `ac2 meas eq …`: EQ what-if sets (`docs/design/eq-overlay.md` §4).

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use super::MeasRef;
use ac2_scene::unit_text::Freq;

/// How a peaking section's Q is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum QMeasuredArg {
    /// Bandwidth between the points at half the dB gain (the cookbook / APO / REW Q).
    HalfGain,
    /// Bandwidth 3 dB below the peak (analog consoles).
    #[value(name = "3db-from-peak")]
    Minus3dbFromPeak,
    /// The band-pass family: the Q of the band-pass it adds.
    BandPass,
}

/// A section's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EqShapeArg {
    /// `F GAIN --q Q | --oct N`.
    Peaking,
    /// `F GAIN --s S | --q Q | --oct N | --first [--corner]`.
    LowShelf,
    /// `F GAIN --s S | --q Q | --oct N | --first [--corner]`.
    HighShelf,
    /// `F --lr N | --bw N | --bessel N [--norm …] | --q Q`.
    HighPass,
    /// `F --lr N | --bw N | --bessel N [--norm …] | --q Q`.
    LowPass,
    /// `F --q Q | --oct N`.
    Notch,
    /// `F --q Q | --oct N`.
    BandPass,
    /// `F` (first order) or `F --q Q` (second order).
    AllPass,
    /// `GAIN`.
    Gain,
    /// Inverts the polarity.
    Polarity,
    /// `TIME`.
    Delay,
    /// `B0 B1 B2 A1 A2 --rate RATE`.
    Biquad,
    /// A stored transfer trace as a section: `--trace NAME`.
    Response,
}

/// Bessel normalisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BesselNormArg {
    /// Asymptotes matched to the Butterworth's (phase-matched).
    Phase,
    /// Unit group delay at DC.
    Delay,
    /// −3 dB at f.
    #[value(name = "3db")]
    Minus3db,
}

/// The text format of `eq export` / `eq import`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EqFormatArg {
    /// ac2 EQ text: the whole set, hand-editable.
    Ac2,
    /// Equalizer APO / REW filter text (`Filter: ON PK Fc …`).
    Apo,
}

/// Which set of a measurement (default: its only one).
#[derive(Debug, Args)]
pub struct SetPick {
    /// The set, by name or id, when the measurement holds several.
    #[arg(long = "set")]
    pub set: Option<MeasRef>,
}

/// The values and options of one section (`eq add`).
#[derive(Debug, Args)]
pub struct EqSectionArgs {
    /// Shape.
    #[arg(value_enum)]
    pub shape: EqShapeArg,
    /// Its values in order (frequency `1.25k` or `80`, gain in dB, a delay `4ms`, biquad
    /// coefficients).
    #[arg(allow_negative_numbers = true)]
    pub values: Vec<String>,
    /// Q (peaking, notch, band-pass: as `--q-measured` reads it; shelves: the cookbook Q;
    /// a high-/low-pass of this Q; a second-order all-pass).
    #[arg(long)]
    pub q: Option<f64>,
    /// Bandwidth in octaves instead of a Q.
    #[arg(long, conflicts_with = "q")]
    pub oct: Option<f64>,
    /// Shelf slope S (1: the steepest without overshoot).
    #[arg(long, conflicts_with_all = ["q", "oct"])]
    pub s: Option<f64>,
    /// First-order shelf.
    #[arg(long, conflicts_with_all = ["q", "oct", "s"])]
    pub first: bool,
    /// The shelf's frequency is its corner (3 dB from the plateau), not its midpoint.
    #[arg(long)]
    pub corner: bool,
    /// Peaking: how this section's Q is read (default: the set's).
    #[arg(long, value_enum)]
    pub q_measured: Option<QMeasuredArg>,
    /// Linkwitz-Riley of this order.
    #[arg(long, value_name = "ORDER")]
    pub lr: Option<u8>,
    /// Butterworth of this order.
    #[arg(long, value_name = "ORDER", conflicts_with = "lr")]
    pub bw: Option<u8>,
    /// Bessel of this order.
    #[arg(long, value_name = "ORDER", conflicts_with_all = ["lr", "bw"])]
    pub bessel: Option<u8>,
    /// Bessel normalisation (default phase).
    #[arg(long, value_enum, requires = "bessel")]
    pub norm: Option<BesselNormArg>,
    /// Biquad: the rate its coefficients are for, e.g. `48k`.
    #[arg(long)]
    pub rate: Option<Freq>,
    /// Response: the stored transfer trace, by name or id.
    #[arg(long)]
    pub trace: Option<MeasRef>,
}

/// `meas eq …`.
#[derive(Debug, Subcommand)]
pub enum MeasEqCmd {
    /// A new set on a measurement (`EQ n` unless named), predicting its live curve.
    New {
        /// Measurement, by name or id.
        meas: MeasRef,
        /// Name.
        #[arg(long)]
        name: Option<String>,
        /// Digital at this rate (default: the session's, 48 kHz without one).
        #[arg(long, conflicts_with = "analog")]
        rate: Option<Freq>,
        /// Analog (the s-domain prototype, no frequency warping).
        #[arg(long)]
        analog: bool,
        /// How peaking Qs are read by default (default half-gain).
        #[arg(long, value_enum)]
        q_measured: Option<QMeasuredArg>,
    },
    /// Add a section (at the end, or before `--at N`).
    Add {
        /// Measurement, by name or id.
        meas: MeasRef,
        #[command(flatten)]
        section: EqSectionArgs,
        /// Insert as section N (1-based).
        #[arg(long)]
        at: Option<usize>,
        #[command(flatten)]
        pick: SetPick,
    },
    /// Change one section by its number: its frequency, gain, Q, or switch it off / on.
    Set {
        /// Measurement, by name or id.
        meas: MeasRef,
        /// Section number (1-based).
        index: usize,
        /// Frequency.
        #[arg(long)]
        f: Option<Freq>,
        /// Gain, dB.
        #[arg(long, allow_negative_numbers = true)]
        gain: Option<f64>,
        /// Q.
        #[arg(long)]
        q: Option<f64>,
        /// Bandwidth, octaves.
        #[arg(long, conflicts_with = "q")]
        oct: Option<f64>,
        /// Skip the section.
        #[arg(long, conflicts_with = "on")]
        off: bool,
        /// Use the section again.
        #[arg(long)]
        on: bool,
        #[command(flatten)]
        pick: SetPick,
    },
    /// Remove a section by its number.
    Rm {
        /// Measurement, by name or id.
        meas: MeasRef,
        /// Section number (1-based).
        index: usize,
        #[command(flatten)]
        pick: SetPick,
    },
    /// Bypass the whole set (`--off`: use it again).
    Bypass {
        /// Measurement, by name or id.
        meas: MeasRef,
        /// End the bypass.
        #[arg(long)]
        off: bool,
        #[command(flatten)]
        pick: SetPick,
    },
    /// Predict a curve of the measurement after the set (`--off`: stop): `live` or a stored
    /// transfer trace's name or id.
    On {
        /// Measurement, by name or id.
        meas: MeasRef,
        /// `live` or a stored trace.
        curve: MeasRef,
        /// Stop predicting it.
        #[arg(long)]
        off: bool,
        #[command(flatten)]
        pick: SetPick,
    },
    /// Mark the set as it is now "in the console": captures from now on record it, and
    /// predictions show only later changes (`--clear`: the console was reset).
    Mark {
        /// Measurement, by name or id.
        meas: MeasRef,
        /// Clear the mark.
        #[arg(long)]
        clear: bool,
        #[command(flatten)]
        pick: SetPick,
    },
    /// Copy the set to another measurement.
    Copy {
        /// Measurement, by name or id.
        meas: MeasRef,
        /// The measurement it is copied to.
        #[arg(long)]
        to: MeasRef,
        #[command(flatten)]
        pick: SetPick,
    },
    /// Delete the set, its filter curve and its predictions.
    Delete {
        /// Measurement, by name or id.
        meas: MeasRef,
        #[command(flatten)]
        pick: SetPick,
    },
    /// The measurement's sets: sections, mark and what each prediction assumes is in.
    Show {
        /// Measurement, by name or id.
        meas: MeasRef,
    },
    /// Write the set as text.
    Export {
        /// Measurement, by name or id.
        meas: MeasRef,
        /// File.
        file: PathBuf,
        /// Format (default ac2).
        #[arg(long, value_enum, default_value = "ac2")]
        format: EqFormatArg,
        /// APO: write what the format expresses and list the rest as comments at the top.
        #[arg(long)]
        skip_unsupported: bool,
        #[command(flatten)]
        pick: SetPick,
    },
    /// Read a set from text: a new set, or into `--set` (which keeps its name).
    Import {
        /// Measurement, by name or id.
        meas: MeasRef,
        /// File.
        file: PathBuf,
        /// Format (default: by content).
        #[arg(long, value_enum)]
        format: Option<EqFormatArg>,
        #[command(flatten)]
        pick: SetPick,
    },
}
