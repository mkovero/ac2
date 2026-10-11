//! `ac2 gen …`: generator signals and options.

use clap::{Args, Subcommand, ValueEnum};

use crate::units::{Channels, Freq, LevelDbfs, SampleCount};

/// `gen …`.
#[derive(Debug, Subcommand)]
pub enum GenCmd {
    /// Pink noise.
    Pink(GenOpts),
    /// White noise.
    White(GenOpts),
    /// Periodic pink noise.
    PeriodicPink {
        /// Period, a power of two, e.g. `65536samples`.
        #[arg(long)]
        period: SampleCount,
        /// Common options.
        #[command(flatten)]
        opts: GenOpts,
    },
    /// Sine.
    Sine {
        /// Frequency, e.g. `1khz`.
        #[arg(long)]
        freq: Freq,
        /// Common options.
        #[command(flatten)]
        opts: GenOpts,
    },
    /// Fade out and disarm whatever is playing (any client, no lease needed).
    Stop,
    /// Show the system max level, or set it (any client). Lowering applies at once: a
    /// stimulus armed or playing above it is stopped. Raising needs `--yes` and nothing
    /// armed or playing; it never goes above the daemon's `--max-level` bound.
    Ceiling {
        /// New system max level, e.g. `-40dbfs`; without it the level in force is shown.
        #[arg(allow_hyphen_values = true, value_name = "DBFS")]
        level: Option<LevelDbfs>,
        /// Confirms a raise.
        #[arg(long)]
        yes: bool,
    },
}

/// Butterworth slope of the noise band limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SlopeArg {
    /// 12 dB/octave.
    #[value(name = "12")]
    Db12,
    /// 24 dB/octave.
    #[value(name = "24")]
    Db24,
}

/// Options of every signal.
#[derive(Debug, Clone, Args)]
pub struct GenOpts {
    /// Output channels, e.g. `1,2`.
    #[arg(long = "out", value_name = "CHANNELS")]
    pub outputs: Channels,
    /// RMS level, e.g. `-20dbfs`. Required: nothing is ever emitted at a default level.
    #[arg(long, allow_hyphen_values = true, value_name = "DBFS")]
    pub level: LevelDbfs,
    /// High-pass corner for noise, e.g. `30hz`.
    #[arg(long)]
    pub hp: Option<Freq>,
    /// Low-pass corner for noise, e.g. `18khz`.
    #[arg(long)]
    pub lp: Option<Freq>,
    /// Band-limit slope, dB/octave.
    #[arg(long, value_enum, default_value = "24")]
    pub slope: SlopeArg,
    /// Take the stimulus over from another client (stops and disarms it first).
    #[arg(long)]
    pub force: bool,
}
