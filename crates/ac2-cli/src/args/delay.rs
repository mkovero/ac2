//! `ac2 delay …`.

use clap::Subcommand;

use super::{BandArg, MeasRef, PickArg, Switch};
use crate::units::{Celsius, DelayAmount, Time};

/// `delay …`.
#[derive(Debug, Subcommand)]
pub enum DelayCmd {
    /// Run the delay finder.
    Find {
        /// Transfer measurement.
        meas: MeasRef,
        /// Band: full, mid, sub, auto, or edges like 80hz-800hz.
        #[arg(long, default_value = "auto")]
        band: BandArg,
        /// Measurement block length (sub band: 2s, 4s or 8s; default: the band's).
        #[arg(long, value_name = "TIME")]
        observation: Option<Time>,
        /// Insert a result right away: first (default), strongest, or a candidate 1 … 3 of
        /// an ambiguous finding.
        #[arg(long, num_args = 0..=1, default_missing_value = "first", value_name = "PICK")]
        insert: Option<PickArg>,
    },
    /// Insert the last finder result.
    Insert {
        /// Transfer measurement.
        meas: MeasRef,
        /// first, strongest, or a candidate 1 … 3 of an ambiguous finding.
        #[arg(long, default_value = "first")]
        pick: PickArg,
    },
    /// Set the delay: `12.5ms`, `600samples` or a distance `4.3m`. The arrival stays: the
    /// live curve alone moves, and the value's distance from it is the offset from the arrival.
    Set {
        /// Transfer measurement.
        meas: MeasRef,
        /// Delay.
        #[arg(allow_hyphen_values = true)]
        delay: DelayAmount,
        /// Air temperature for distances.
        #[arg(long, default_value = "20c", allow_hyphen_values = true)]
        temp: Celsius,
    },
    /// Move the delay by a step: `1sample`, `-0.25samples`, `-20us`, `1cm`. The curve moves
    /// at once; the averages are kept where the change is small next to each analysis window.
    Nudge {
        /// Transfer measurement.
        meas: MeasRef,
        /// Step (either sign for time and samples).
        #[arg(allow_hyphen_values = true)]
        by: DelayAmount,
        /// Air temperature for distances.
        #[arg(long, default_value = "20c", allow_hyphen_values = true)]
        temp: Celsius,
    },
    /// Track the delay continuously.
    Track {
        /// Transfer measurement.
        meas: MeasRef,
        /// on or off.
        #[arg(value_enum)]
        state: Switch,
    },
}
