//! `ac2 sim …`: a room simulation of a plan (image sources and ray tracing in the daemon).

use clap::{Args, Subcommand};

use ac2_proto::model::{SimMapRequest, SimSettings, WalkPointSpec};
use ac2_proto::units::{Hz, Seconds};

use super::MeasRef;

/// `sim …`.
#[derive(Debug, Subcommand)]
pub enum SimCmd {
    /// Simulate a plan's speakers at a walk's points or at given points, with the plan's
    /// materials (absorption assumed, or fitted by an earlier `sim fit` on the same plan).
    /// Waits for the result unless `--detach`.
    Run(SimRunArgs),
    /// Fit the plan's absorption per octave band so the simulated T20 at a sweep walk's
    /// captured points matches the measured one.
    Fit(SimFitArgs),
    /// A coverage map: the direct and early energy of the plan's speakers on a grid over its
    /// zones (image sources only), dB re its loudest cell.
    Map(SimMapArgs),
    /// The last simulation: its progress, or its result.
    Status,
    /// Stop the running simulation.
    Cancel,
}

/// `sim run`.
#[derive(Debug, Args)]
pub struct SimRunArgs {
    /// The plan (the session's copy, else the library's).
    #[arg(long)]
    pub plan: String,
    /// A speaker marker of the plan (repeatable); every speaker when left out.
    #[arg(long = "source", value_name = "SPEAKER")]
    pub sources: Vec<String>,
    /// Receivers at this walk's points.
    #[arg(long, conflicts_with = "points", required_unless_present = "points")]
    pub walk: Option<MeasRef>,
    /// A receiver: `x,y,z` in metres, or `label=x,y,z` (repeatable; unlabelled ones are
    /// P1, P2, … in order).
    #[arg(long = "point", value_name = "[LABEL=]X,Y,Z", value_parser = parse_point)]
    pub points: Vec<(Option<String>, [f64; 3])>,
    #[command(flatten)]
    pub settings: SimFlags,
    /// Start it and return; `ac2 sim status` shows how it goes.
    #[arg(long)]
    pub detach: bool,
}

/// `sim fit`.
#[derive(Debug, Args)]
pub struct SimFitArgs {
    /// The sweep walk whose measured T20 the fit matches; the session's only walk when left
    /// out.
    #[arg(long)]
    pub walk: Option<MeasRef>,
    /// The speaker marker the walk was measured from; the plan's only speaker when left out.
    #[arg(long)]
    pub source: Option<String>,
    #[command(flatten)]
    pub settings: SimFlags,
    /// Start it and return; `ac2 sim status` shows how it goes.
    #[arg(long)]
    pub detach: bool,
}

/// `sim map`.
#[derive(Debug, Args)]
pub struct SimMapArgs {
    /// The plan (the session's copy, else the library's).
    #[arg(long)]
    pub plan: String,
    /// A speaker marker of the plan (repeatable; their energies add); every speaker when
    /// left out.
    #[arg(long = "source", value_name = "SPEAKER")]
    pub sources: Vec<String>,
    /// Grid spacing, metres.
    #[arg(long, default_value_t = SimMapRequest::DEFAULT_SPACING_M)]
    pub spacing: f64,
    /// The octave band mapped, Hz (125 … 8000); every band summed when left out.
    #[arg(long)]
    pub band: Option<f64>,
    /// Energy counted after the direct sound, milliseconds.
    #[arg(long = "window-ms", default_value_t = SimMapRequest::DEFAULT_WINDOW_S * 1000.0)]
    pub window_ms: f64,
    /// Reflection order of the image sources.
    #[arg(long, default_value_t = SimMapRequest::DEFAULT_IMAGE_ORDER,
          value_parser = clap::value_parser!(u8).range(0..=i64::from(SimSettings::MAX_IMAGE_ORDER)))]
    pub order: u8,
    /// Start it and return; `ac2 sim status` shows how it goes.
    #[arg(long)]
    pub detach: bool,
}

impl SimMapArgs {
    /// The request, with `sources` when none were named.
    pub fn request(&self, sources: Vec<String>) -> SimMapRequest {
        SimMapRequest {
            plan: self.plan.clone(),
            sources,
            spacing: self.spacing,
            band: self.band.map(Hz),
            window: Seconds(self.window_ms / 1000.0),
            image_order: self.order,
        }
    }
}

/// How a simulation runs.
#[derive(Debug, Args)]
pub struct SimFlags {
    /// Rays traced per octave band (per source); fewer when the budget runs out first.
    #[arg(long, default_value_t = SimSettings::default().rays_per_band,
          value_parser = clap::value_parser!(u32).range(1..=i64::from(SimSettings::MAX_RAYS)))]
    pub rays: u32,
    /// Reflection order of the image sources (the early reflections).
    #[arg(long, default_value_t = SimSettings::default().image_order,
          value_parser = clap::value_parser!(u8).range(0..=i64::from(SimSettings::MAX_IMAGE_ORDER)))]
    pub order: u8,
    /// Time budget of the ray tracing, seconds (per fit pass).
    #[arg(long, default_value_t = SimSettings::default().budget.0)]
    pub budget: f64,
    /// Seed of the ray directions: the same seed, the same result.
    #[arg(long, default_value_t = SimSettings::default().seed)]
    pub seed: u64,
}

impl SimFlags {
    /// The typed settings.
    pub fn settings(&self) -> SimSettings {
        SimSettings {
            image_order: self.order,
            rays_per_band: self.rays,
            budget: Seconds(self.budget),
            seed: self.seed,
        }
    }
}

impl SimRunArgs {
    /// The receivers given by `--point`, labelled.
    pub fn point_specs(&self) -> Vec<WalkPointSpec> {
        self.points
            .iter()
            .enumerate()
            .map(|(i, (label, at))| WalkPointSpec {
                label: label.clone().unwrap_or_else(|| format!("P{}", i + 1)),
                at: *at,
            })
            .collect()
    }
}

/// `x,y,z` or `label=x,y,z`.
fn parse_point(s: &str) -> Result<(Option<String>, [f64; 3]), String> {
    let (label, xyz) = match s.split_once('=') {
        Some((l, r)) if !l.trim().is_empty() => (Some(l.trim().to_owned()), r),
        Some(_) => return Err("an empty label".into()),
        None => (None, s),
    };
    let v: Vec<f64> = xyz
        .split(',')
        .map(|c| c.trim().parse::<f64>().map_err(|e| format!("{c:?}: {e}")))
        .collect::<Result<_, _>>()?;
    match v.as_slice() {
        [x, y, z] if v.iter().all(|c| c.is_finite()) => Ok((label, [*x, *y, *z])),
        _ => Err(format!("{xyz:?}: three coordinates in metres, x,y,z")),
    }
}
