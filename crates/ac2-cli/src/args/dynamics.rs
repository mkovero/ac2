//! `ac2 dynamics …`.

use clap::{Args, Subcommand};

use super::MeasRef;

/// `dynamics …`.
#[derive(Debug, Subcommand)]
pub enum DynamicsCmd {
    /// Run a dynamics measurement in the foreground with its settings (Enter plays, Esc/Ctrl-C
    /// stops); the run is stored as one dynamics trace under it, and its step table printed.
    Run(DynamicsRunArgs),
}

/// `dynamics run`.
#[derive(Debug, Args)]
pub struct DynamicsRunArgs {
    /// The dynamics measurement, by id or name.
    pub meas: MeasRef,
    /// Name of the stored run (default: `Run <number>`).
    #[arg(long)]
    pub name: Option<String>,
    /// Replay the rests of this dynamics trace (id or name) instead of adapting them, so an
    /// A/B pair plays an identical programme.
    #[arg(long, value_name = "TRACE")]
    pub rests_from: Option<MeasRef>,
    /// Take the stimulus lease over from another client.
    #[arg(long)]
    pub force: bool,
}
