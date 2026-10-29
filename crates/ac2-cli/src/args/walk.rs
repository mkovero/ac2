//! `ac2 plan …` and `ac2 walk …`: room plans in the daemon's library, and spatial walks
//! through their positions.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use super::MeasRef;

/// `plan …`.
#[derive(Debug, Subcommand)]
pub enum PlanCmd {
    /// A rectangular room with one audience zone 1 m in from its walls, saved to the
    /// library.
    New(PlanNew),
    /// The plans in the library.
    List,
    /// One plan: its outline, zones and markers.
    Show {
        /// Plan name.
        plan: String,
    },
    /// Save a plan file (JSON, as `plan export` writes it) to the library.
    Import {
        /// The file.
        file: PathBuf,
        /// Save it under this name instead of the file's.
        #[arg(long)]
        name: Option<String>,
    },
    /// Write a plan as JSON.
    Export {
        /// Plan name.
        plan: String,
        /// Where; standard output when left out.
        file: Option<PathBuf>,
    },
}

/// `plan new`.
#[derive(Debug, Args)]
pub struct PlanNew {
    /// Plan name (a file name: letters, digits, space, `-`, `_`, `.`).
    pub name: String,
    /// Room width (plan x), metres.
    #[arg(long)]
    pub width: f64,
    /// Room depth (plan y, away from the stage), metres.
    #[arg(long)]
    pub depth: f64,
    /// Ceiling height, metres; left out: outdoors.
    #[arg(long)]
    pub ceiling: Option<f64>,
    /// The audience's ear height, metres.
    #[arg(long, default_value_t = 1.2)]
    pub ear_height: f64,
    /// Seats in the audience zone (sets the ISO 3382-1 position count).
    #[arg(long)]
    pub seats: Option<u32>,
}

/// `walk …`. Without `--walk`, the session's only walk.
#[derive(Debug, Subcommand)]
pub enum WalkCmd {
    /// Start a walk: positions suggested over a plan's zones and grouped into steps that
    /// place every mic once, the measurements restarted at the first.
    New(WalkNew),
    /// The walk's points, where the mics are, and what was captured.
    Status {
        /// The walk, by id or name.
        #[arg(long)]
        walk: Option<MeasRef>,
    },
    /// Capture where the mics stand and go on to the next step with a pending point.
    Capture {
        /// The walk, by id or name.
        #[arg(long)]
        walk: Option<MeasRef>,
    },
    /// Go to the next step (the measurements restart there).
    Next {
        /// The walk, by id or name.
        #[arg(long)]
        walk: Option<MeasRef>,
    },
    /// Go back to the previous step.
    Prev {
        /// The walk, by id or name.
        #[arg(long)]
        walk: Option<MeasRef>,
    },
    /// Leave a point out: the one named, else every point a mic stands at; the walk goes on
    /// when its step has none left.
    Skip {
        /// The point, by label (`P3`) or number (1 is the first).
        point: Option<String>,
        /// The walk, by id or name.
        #[arg(long)]
        walk: Option<MeasRef>,
    },
    /// Capture a point again: its old capture is kept, out of the average, and its mic
    /// goes there.
    Redo {
        /// The point, by label (`P3`) or number (1 is the first).
        point: String,
        /// The walk, by id or name.
        #[arg(long)]
        walk: Option<MeasRef>,
    },
    /// The mics standing at these points of the step move round by one: each to the next
    /// point named, the last to the first (two points: they trade places).
    Swap {
        /// The points, by label (`P3`) or number (1 is the first); at least two.
        #[arg(num_args = 2.., required = true)]
        points: Vec<String>,
        /// The walk, by id or name.
        #[arg(long)]
        walk: Option<MeasRef>,
    },
}

/// `walk new`.
#[derive(Debug, Args)]
pub struct WalkNew {
    /// Walk name.
    pub name: String,
    /// The plan to walk (from the library, or the session's copy).
    #[arg(long)]
    pub plan: String,
    /// The transfer measurements that capture, one per mic, each on its own input
    /// (`--meas A --meas B` for two mics); or one sweep measurement, whose measurement
    /// inputs are the mics: a sweep walk, captured by `ac2 sweep run`.
    #[arg(long, value_name = "MEAS", required = true)]
    pub meas: Vec<MeasRef>,
    /// Which rules place the positions.
    #[arg(long, value_enum, default_value_t = PatternArg::Tuning)]
    pub preset: PatternArg,
    /// Reverberation time, seconds (`--preset iso3382`: sets the source distance).
    #[arg(long, value_name = "SECONDS", required_if_eq("preset", "iso3382"))]
    pub rt: Option<f64>,
    /// Number of positions (default: the preset's, 8 for tuning).
    #[arg(long, short = 'n')]
    pub points: Option<usize>,
    /// The zones to place them in, by name; all when left out.
    #[arg(long = "zone", value_name = "ZONE")]
    pub zones: Vec<String>,
    /// Seed of the suggestion: another seed, another layout under the same rules.
    #[arg(long, default_value_t = 1)]
    pub seed: u64,
}

/// A suggestion preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PatternArg {
    /// Tuning a system: the audience area, front rows included.
    Tuning,
    /// ISO 3382-1 room parameters.
    Iso3382,
}
