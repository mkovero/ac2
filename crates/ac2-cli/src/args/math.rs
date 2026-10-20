//! `ac2 math …`: math channels and their operands.

use std::str::FromStr;

use clap::{Args, Subcommand, ValueEnum};

use super::{AverageArg, FractionArg, MeasRef};
use ac2_scene::unit_text::{Freq, Time};

/// `math …`.
#[derive(Debug, Subcommand)]
pub enum MathCmd {
    /// Create a math channel: `ac2 math new "Main L / Sub"`, `--op div --a "Main L" --b
    /// Sub`, or `--op avg --of "Seat 1,Seat 2,Seat 3"`. It is listed in `meas list` and
    /// captured like any measurement (`trace capture`).
    New {
        /// The channel.
        #[command(flatten)]
        math: MathArgs,
        /// Start right away.
        #[arg(long)]
        start: bool,
    },
    /// Change a math channel: its operands and operator (as for `new`), its average method,
    /// phase reference, smoothing or name. What is not given stays as it is.
    Set {
        /// The math channel, by id or name.
        channel: MeasRef,
        /// What changes.
        #[command(flatten)]
        math: MathArgs,
    },
}

/// A math channel as typed.
#[derive(Debug, Args)]
pub struct MathArgs {
    /// `"A op B"` with the operator spaced: `/` or `÷` (A relative to B), `*` or `×`
    /// (cascade), `+` (sum), `-` or `−` (difference). Operands by name or id; `trace:NAME`
    /// or `meas:NAME` when a measurement and a trace share a name.
    pub expr: Option<String>,
    /// The operator, with `--a`/`--b` (or `--of` for `avg`).
    #[arg(long, value_enum, conflicts_with = "expr")]
    pub op: Option<MathOpArg>,
    /// Operand A.
    #[arg(long, value_name = "OPERAND", conflicts_with = "expr")]
    pub a: Option<MeasRef>,
    /// Operand B.
    #[arg(long, value_name = "OPERAND", conflicts_with = "expr")]
    pub b: Option<MeasRef>,
    /// avg: the operands averaged, e.g. `--of "Seat 1,Seat 2,Seat 3"`.
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "OPERAND,…",
        conflicts_with = "expr"
    )]
    pub of: Vec<MeasRef>,
    /// avg: power (RMS magnitude), complex, or coherence (inverse-variance weighted);
    /// spectra and RTA: power only.
    #[arg(long, value_enum)]
    pub method: Option<AverageArg>,
    /// Phase reference of a sum, difference or average: this operand's delay (default: the
    /// first operand).
    #[arg(long, value_name = "OPERAND", conflicts_with = "ref_delay")]
    pub phase_ref: Option<MeasRef>,
    /// Phase reference: an explicit delay, e.g. `12.5ms`.
    #[arg(long, value_name = "TIME", allow_hyphen_values = true)]
    pub ref_delay: Option<Time>,
    /// Display smoothing of the result, 1/N octave (transfer and spectrum math).
    #[arg(long, value_enum, conflicts_with = "no_smooth")]
    pub smooth: Option<FractionArg>,
    /// Transfer math: smooth the magnitude only and keep the phase.
    #[arg(long, requires = "smooth")]
    pub smooth_magnitude_only: bool,
    /// `set`: take the smoothing off.
    #[arg(long)]
    pub no_smooth: bool,
    /// Name (default for `new`: the expression, e.g. `Main L ÷ Sub`).
    #[arg(long)]
    pub name: Option<String>,
    /// The measurement it is listed under (default for `new`: its first operand when that is
    /// a measurement, else where that trace is filed).
    #[arg(long, value_name = "MEAS", conflicts_with = "imported")]
    pub under: Option<MeasRef>,
    /// List it in the imported group, under no measurement.
    #[arg(long)]
    pub imported: bool,
}

/// A math operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MathOpArg {
    /// A ÷ B: A relative to B (transfer).
    Div,
    /// A × B: cascade (transfer).
    Mul,
    /// A + B: transfer: complex sum; levels: power sum.
    Add,
    /// A − B: transfer: complex difference; levels: level difference.
    Sub,
    /// The average of `--of`.
    Avg,
}

/// Which finder result to insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickArg {
    /// First arrival (the default); of an ambiguous finding, the rule's pre-selection.
    First,
    /// Strongest peak.
    Strongest,
    /// Entry N (1, 2, 3) of an ambiguous finding's candidate list, as printed.
    Ranked(u8),
}

impl FromStr for PickArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "first" | "first-arrival" => Ok(Self::First),
            "strongest" => Ok(Self::Strongest),
            n => match n.parse::<u8>() {
                Ok(i @ 1..=3) => Ok(Self::Ranked(i)),
                _ => Err(format!(
                    "{s:?}: expected first, strongest or a candidate number 1 … 3"
                )),
            },
        }
    }
}

/// Delay-finder band: `full`, `mid`, `sub`, `auto` or custom edges `80hz-800hz`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BandArg {
    /// 2 – 16 kHz.
    Full,
    /// 300 Hz – 3 kHz.
    Mid,
    /// 20 – 120 Hz.
    Sub,
    /// Full → mid → sub, the first that is not refused.
    Auto,
    /// Operator edges.
    Custom(Freq, Freq),
}

impl FromStr for BandArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "mid" => Ok(Self::Mid),
            "sub" => Ok(Self::Sub),
            "auto" => Ok(Self::Auto),
            edges => {
                let bad =
                    || format!("{s:?}: expected full, mid, sub, auto or edges like 80hz-800hz");
                let (lo, hi) = edges.split_once('-').ok_or_else(bad)?;
                let lo: Freq = lo
                    .parse()
                    .map_err(|e: ac2_scene::unit_text::UnitError| e.0)?;
                let hi: Freq = hi
                    .parse()
                    .map_err(|e: ac2_scene::unit_text::UnitError| e.0)?;
                if hi.0.0 <= lo.0.0 {
                    return Err(format!("{s:?}: the upper edge must be above the lower"));
                }
                Ok(Self::Custom(lo, hi))
            }
        }
    }
}

/// On or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Switch {
    /// Enable.
    On,
    /// Disable.
    Off,
}
