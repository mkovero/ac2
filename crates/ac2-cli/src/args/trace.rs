//! `ac2 trace …`: stored traces and their display settings.

use std::path::PathBuf;
use std::str::FromStr;

use clap::{Subcommand, ValueEnum};

use super::{AverageArg, ImportFormatArg, MeasRef};
use crate::units::{Celsius, Time};

/// `trace …`.
#[derive(Debug, Subcommand)]
pub enum TraceCmd {
    /// Capture a measurement's live result (transfer, spectrum or RTA) with its metadata.
    Capture {
        /// Source measurement.
        meas: MeasRef,
        /// Trace name.
        #[arg(long)]
        name: String,
        /// Put it in slot 1 … 9 (taken from the trace holding it).
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=9))]
        slot: Option<u8>,
    },
    /// List traces.
    List,
    /// One trace's metadata (with `--data`, its columns too).
    Show {
        /// Trace id or name.
        trace: MeasRef,
        /// Include frequency, magnitude, phase and coherence columns.
        #[arg(long)]
        data: bool,
    },
    /// Delete traces.
    Rm {
        /// Trace ids or names.
        #[arg(required = true)]
        traces: Vec<MeasRef>,
    },
    /// File traces under another measurement, or in the imported group (`--imported`).
    Move {
        /// Trace ids or names.
        #[arg(required = true)]
        traces: Vec<MeasRef>,
        /// The measurement to file them under.
        #[arg(long, value_name = "MEAS", conflicts_with = "imported")]
        to: Option<MeasRef>,
        /// The imported group, under no measurement.
        #[arg(long)]
        imported: bool,
    },
    /// Average traces into a new trace.
    Average {
        /// Trace ids or names (at least two).
        #[arg(required = true, num_args = 2..)]
        traces: Vec<MeasRef>,
        /// Name of the result.
        #[arg(long)]
        name: String,
        /// power (RMS magnitude), complex, or coherence (inverse-variance weighted).
        #[arg(long, value_enum, default_value = "power")]
        method: AverageArg,
        /// Phase reference: this trace's measured delay (default: the first listed).
        #[arg(long = "ref", value_name = "TRACE", conflicts_with = "ref_delay")]
        reference: Option<MeasRef>,
        /// Phase reference: an explicit delay, e.g. `12.5ms`.
        #[arg(long, value_name = "TIME", allow_hyphen_values = true)]
        ref_delay: Option<Time>,
    },
    /// Import a CSV / analyzer text export (freq, mag[, phase][, coherence]), or an
    /// impulse response as a WAV / RF64 file (16/24/32-bit integer or 32-bit float, any
    /// rate): a transfer trace with the IR and its frequency response.
    Import {
        /// File.
        file: PathBuf,
        /// Import as a target curve (magnitude only).
        #[arg(long)]
        target: bool,
        /// File format.
        #[arg(long, value_enum, default_value = "auto")]
        format: ImportFormatArg,
        /// Name (default: from the file).
        #[arg(long)]
        name: Option<String>,
        /// IR: the channel of a multi-channel WAV file, 1-based.
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..))]
        channel: Option<u16>,
        /// IR: window the response is computed through, this long after the peak (default:
        /// the whole file, unwindowed).
        #[arg(long, value_name = "TIME")]
        gate: Option<Time>,
        /// IR: with `--gate`, this long before the peak.
        #[arg(long, value_name = "TIME", default_value = "1ms", requires = "gate")]
        gate_pre: Time,
        /// IR: keep the file's first sample as time zero (the phase keeps the whole delay)
        /// instead of removing the delay up to the peak.
        #[arg(long)]
        keep_delay: bool,
        /// IR: columns per octave of the response: 1/12, 1/24, 1/48 (default) or 1/96.
        #[arg(long)]
        resolution: Option<ResolutionArg>,
    },
    /// Export a trace: its columns as ac2 CSV, or its impulse response as a mono 32-bit
    /// float WAV (sweeps, transfer captures, imported IRs).
    Export {
        /// Trace id or name.
        trace: MeasRef,
        /// Write ac2 CSV to this file (`-` = stdout).
        #[arg(
            long,
            value_name = "FILE",
            required_unless_present = "wav",
            conflicts_with = "wav"
        )]
        csv: Option<PathBuf>,
        /// Write the impulse response as WAV to this file (`-` = stdout).
        #[arg(long, value_name = "FILE")]
        wav: Option<PathBuf>,
    },
    /// Set a trace's display smoothing (transfer, sweep and spectrum traces): the stored
    /// columns stay as measured, so it can be changed or removed at any time.
    Smooth {
        /// Trace id or name.
        trace: MeasRef,
        /// 1/N octave: `1/3`, `1/6`, `1/12`, `1/24`, `1/48` (or just N), or `none`.
        fraction: TraceSmoothing,
        /// Smooth the phase too (complex smoothing). Default: the trace's current mode,
        /// magnitude and phase for an unsmoothed trace.
        #[arg(long, conflicts_with = "magnitude_only")]
        phase: bool,
        /// Smooth the magnitude only and keep the measured phase.
        #[arg(long)]
        magnitude_only: bool,
    },
    /// Show or hide a trace in the app's panes (`on` shows, `off` hides).
    Display {
        /// Trace id or name.
        trace: MeasRef,
        /// `on` or `off`.
        state: Shown,
    },
    /// The difference of two traces' delays, A − B, to 0.1 µs: for sweeps, the difference
    /// of their arrivals (what aligning a pair trims); for captures, of the applied delays.
    DelayDiff {
        /// Trace A (id or name).
        a: MeasRef,
        /// Trace B (id or name).
        b: MeasRef,
        /// Air temperature for the path-length difference.
        #[arg(long, default_value = "20c", allow_hyphen_values = true)]
        temp: Celsius,
    },
    /// Rename a stored trace.
    Rename {
        /// Trace id or name.
        trace: MeasRef,
        /// The new name.
        name: String,
    },
    /// Put a trace in slot 1 … 9 (the trace holding it gives it up), or `none` to free its
    /// slot.
    Slot {
        /// Trace id or name.
        trace: MeasRef,
        /// 1 … 9, or `none`.
        slot: TraceSlot,
    },
    /// Apply a curve of the mic library to a stored trace (a display edit: the columns stay
    /// as measured), or `none` to remove it. Refused for a trace captured with a curve
    /// already in its columns.
    Mic {
        /// Trace id or name.
        trace: MeasRef,
        /// Mic name (e.g. "MM1 34804"), or `none`.
        mic: String,
        /// Which of the mic's curves (e.g. 90°); may be left out when it has only one.
        #[arg(long)]
        label: Option<String>,
    },
}

/// Whether a trace is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Shown {
    /// Drawn.
    On,
    /// Hidden.
    Off,
}

/// A slot argument: `1` … `9`, or `none` (`off`, `-`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceSlot(pub Option<u8>);

impl FromStr for TraceSlot {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let t = s.trim().to_ascii_lowercase();
        if matches!(t.as_str(), "none" | "off" | "-") {
            return Ok(Self(None));
        }
        match t.parse::<u8>() {
            Ok(n @ 1..=9) => Ok(Self(Some(n))),
            _ => Err(format!("{s:?}: a slot is 1 … 9, or none")),
        }
    }
}

/// A Resolution argument: `1/N` or `N`, N one of 12, 24, 48, 96; for a spectrum `multi`
/// or `multi:1/N` (the multi-resolution bands on a 1/N-octave grid, 1/48 by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionArg {
    /// Points per octave.
    pub grid: ac2_proto::model::Resolution,
    /// `multi[:1/N]`: a spectrum's multi-resolution bands.
    pub multi: bool,
}

impl ResolutionArg {
    /// `1/N` without `multi`.
    pub fn grid(grid: ac2_proto::model::Resolution) -> Self {
        Self { grid, multi: false }
    }

    /// The points per octave of a tf or sweep, which have no `multi`.
    pub fn columns(self) -> Result<ac2_proto::model::Resolution, String> {
        if self.multi {
            Err("--resolution multi is a spectrum's; tf and sweep take 1/N".into())
        } else {
            Ok(self.grid)
        }
    }
}

impl FromStr for ResolutionArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let t = s.trim();
        let (multi, rest) = match t.strip_prefix("multi") {
            Some("") => (true, None),
            Some(r) => match r.strip_prefix(':') {
                Some(r) => (true, Some(r)),
                None => (false, Some(t)),
            },
            None => (false, Some(t)),
        };
        let grid = match rest {
            None => ac2_proto::model::Resolution::FortyEighth,
            Some(r) => r
                .strip_prefix("1/")
                .unwrap_or(r)
                .parse::<u32>()
                .ok()
                .and_then(ac2_proto::model::Resolution::from_ppo)
                .ok_or_else(|| {
                    format!(
                        "{s:?}: the resolution is 1/12, 1/24, 1/48 or 1/96 octave \
                         (a spectrum: multi or multi:1/N)"
                    )
                })?,
        };
        Ok(Self { grid, multi })
    }
}

/// A measurement's colour: `1` … `8` (the families, as the app names them `colour N`),
/// `grey`, or `auto` (the one its id gives).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColourArg(pub Option<ac2_proto::model::ColourFamily>);

impl FromStr for ColourArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        use ac2_proto::model::ColourFamily;
        let t = s.trim().to_ascii_lowercase();
        match t.as_str() {
            "auto" => return Ok(Self(None)),
            "grey" | "gray" => return Ok(Self(Some(ColourFamily::Neutral))),
            _ => {}
        }
        t.parse::<u8>()
            .ok()
            .filter(|n| (1..=ColourFamily::HUES).contains(n))
            .map(|n| Self(Some(ColourFamily::Hue { index: n - 1 })))
            .ok_or_else(|| {
                format!(
                    "{s:?}: a colour is 1 … {}, grey or auto",
                    ColourFamily::HUES
                )
            })
    }
}

/// A trace smoothing argument: `1/N`, `N` or `none` (`off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceSmoothing(pub Option<ac2_proto::model::SmoothingFraction>);

impl FromStr for TraceSmoothing {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        use ac2_proto::model::SmoothingFraction;
        let t = s.trim().to_ascii_lowercase();
        if t == "none" || t == "off" {
            return Ok(Self(None));
        }
        let n = t.strip_prefix("1/").unwrap_or(&t);
        Ok(Self(Some(match n {
            "3" => SmoothingFraction::Third,
            "6" => SmoothingFraction::Sixth,
            "12" => SmoothingFraction::Twelfth,
            "24" => SmoothingFraction::TwentyFourth,
            "48" => SmoothingFraction::FortyEighth,
            _ => {
                return Err(format!(
                    "{s:?}: smoothing is 1/3, 1/6, 1/12, 1/24, 1/48 octave or none"
                ));
            }
        })))
    }
}
