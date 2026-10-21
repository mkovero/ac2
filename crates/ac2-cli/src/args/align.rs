//! `ac2 align …`: two transfer curves aligned in their overlap.

use std::str::FromStr;

use clap::{Args, Subcommand};

use super::MeasRef;
use ac2_proto::model::AlignBand;
use ac2_scene::unit_text::{Celsius, Freq, Time, UnitText};

/// `align …`.
#[derive(Debug, Subcommand)]
pub enum AlignCmd {
    /// Align two transfer curves (live or stored) in their overlap: which one to delay, by
    /// how much, the polarity and how fully they then add. Created as a math channel
    /// (`ac2 math set` renames it), started, and its first fit printed.
    New(AlignNew),
    /// The current fit of an alignment.
    Show {
        /// The alignment, by id or name.
        alignment: MeasRef,
        /// Air temperature for distances.
        #[arg(long, default_value = "20c", allow_hyphen_values = true)]
        temp: Celsius,
    },
}

/// `align new`.
#[derive(Debug, Args)]
pub struct AlignNew {
    /// System A, a transfer curve by name or id: a measurement's live curve or a stored
    /// trace (`trace:NAME` or `meas:NAME` when both are named alike).
    pub a: MeasRef,
    /// System B.
    pub b: MeasRef,
    /// The overlap band: `60..120`, `800..1.8k` (Hz). Asked, never assumed.
    #[arg(long, value_name = "FROM..TO")]
    pub band: OverlapArg,
    /// The longest delay looked at.
    #[arg(long, default_value = "50ms")]
    pub max_delay: Time,
    /// Name (default `A ↔ B`).
    #[arg(long)]
    pub name: Option<String>,
    /// The measurement it is listed under (default: A's).
    #[arg(long, value_name = "MEAS", conflicts_with = "imported")]
    pub under: Option<MeasRef>,
    /// List it in the imported group.
    #[arg(long)]
    pub imported: bool,
    /// Air temperature for distances.
    #[arg(long, default_value = "20c", allow_hyphen_values = true)]
    pub temp: Celsius,
}

/// An overlap band, `FROM..TO` in Hz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlapArg(pub AlignBand);

impl FromStr for OverlapArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (lo, hi) = s
            .split_once("..")
            .ok_or_else(|| format!("{s:?}: write the band as FROM..TO, e.g. 60..120"))?;
        let hz = |t: &str| {
            Freq::parse_text(t.trim(), Some("hz"))
                .map(|f| f.0)
                .map_err(|e| e.0)
        };
        let band = AlignBand {
            lo: hz(lo)?,
            hi: hz(hi)?,
        };
        if !band.is_valid() {
            return Err(format!("{s:?}: the band needs 0 < FROM < TO"));
        }
        Ok(Self(band))
    }
}
