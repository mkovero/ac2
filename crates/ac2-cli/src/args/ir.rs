//! `ac2 ir …`, `ac2 rec …` and `ac2 sweep …`.

use clap::{Args, Subcommand, ValueEnum};

use super::{LfHarmonicsArg, MeasRef, ResolutionArg};
use ac2_scene::unit_text::{ByteSize, Channel, Channels, Freq, LevelDbfs, Time};

/// `ir …`.
#[derive(Debug, Subcommand)]
pub enum IrCmd {
    /// Play a synchronised sweep in the foreground (Enter plays, Esc/Ctrl-C stops), record
    /// the reference and the mic, and store a sweep trace with harmonic distortion H2 … H5
    /// and THD vs frequency, and the room parameters (ISO 3382-1) of its impulse response;
    /// prints a summary.
    Capture(IrCaptureArgs),
    /// Room parameters (ISO 3382-1: EDT, T20, T30, C50, C80, D50) of a stored sweep trace,
    /// per octave band (or one-third octave) and broadband.
    Metrics(IrMetricsArgs),
}

/// `ir metrics`.
#[derive(Debug, Args)]
pub struct IrMetricsArgs {
    /// Sweep trace id or name.
    pub trace: MeasRef,
    /// One-third-octave bands instead of octave bands.
    #[arg(long)]
    pub third: bool,
}

/// `rec …`.
#[derive(Debug, Subcommand)]
pub enum RecCmd {
    /// Start recording inputs of the open session (f32 WAV + JSON sidecar on the daemon
    /// host); it runs until `rec stop`, its time or size limit, or the session closes.
    Start(RecStartArgs),
    /// Stop the recording and finalise its file.
    Stop,
    /// The recording in progress, or the last one.
    Status,
    /// Recordings in the daemon's recording directory.
    List,
    /// Turn a WAV from elsewhere (a recorder's 16-, 24- or 32-bit PCM or float file) into a
    /// recording `session replay` plays; written on this computer, no daemon needed.
    Import(RecImportArgs),
}

/// `rec import`.
#[derive(Debug, Args)]
pub struct RecImportArgs {
    /// The WAV file.
    pub file: std::path::PathBuf,
    /// Recording name (default: the file's name without its extension).
    #[arg(long)]
    pub name: Option<String>,
    /// Directory to write it to (default: the WAV's own directory; for a local daemon's
    /// `rec list`, its recording directory).
    #[arg(long, value_name = "DIR")]
    pub dir: Option<std::path::PathBuf>,
}

/// `rec start`.
#[derive(Debug, Args)]
pub struct RecStartArgs {
    /// Inputs to record, e.g. `1,2` (default: every input of the session).
    #[arg(long = "in", value_name = "CHANNELS")]
    pub inputs: Option<Channels>,
    /// File name (default `rec-<UTC date and time>`); an existing recording is never
    /// replaced.
    #[arg(long)]
    pub name: Option<String>,
    /// Stop by itself after this much audio, e.g. `10min`, `1h`; required.
    #[arg(long, value_name = "TIME")]
    pub max: Time,
    /// … or once the file reaches this size, e.g. `2GB`.
    #[arg(long, value_name = "SIZE")]
    pub max_size: Option<ByteSize>,
}

/// `ir capture`.
#[derive(Debug, Args)]
pub struct IrCaptureArgs {
    /// Reference (loopback) input, e.g. `2`.
    #[arg(
        long = "ref",
        value_name = "IN",
        required_unless_present = "meas",
        conflicts_with = "meas",
        requires = "mic"
    )]
    pub reference: Option<Channel>,
    /// Measurement (mic) input, e.g. `1`.
    #[arg(long, value_name = "IN", requires = "reference")]
    pub mic: Option<Channel>,
    /// Take both inputs from this transfer measurement instead.
    #[arg(long, value_name = "MEAS")]
    pub meas: Option<MeasRef>,
    /// Outputs playing the sweep: the speaker's and the loopback's, e.g. `1,2`.
    #[arg(long = "out", value_name = "OUTS")]
    pub outputs: Channels,
    /// RMS level of the sweep, e.g. `-50dbfs`; required (there is no default level).
    #[arg(long, allow_hyphen_values = true)]
    pub level: LevelDbfs,
    /// Start frequency.
    #[arg(long, default_value = "20hz")]
    pub from: Freq,
    /// End frequency.
    #[arg(long, default_value = "20khz")]
    pub to: Freq,
    /// Sweep duration (rounded so the harmonics stay in phase); longer sweeps resolve lower
    /// frequencies and lower the noise floor.
    #[arg(long, default_value = "3s")]
    pub duration: Time,
    /// Sweeps played and averaged (1 … 8): each doubling lowers the noise floor by 3 dB.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=8))]
    pub repeats: u8,
    /// Gate the linear response this long after the arrival (e.g. `5ms` for the free-field
    /// response); default: the whole response.
    #[arg(long, value_name = "TIME")]
    pub gate: Option<Time>,
    /// Silence recorded after each sweep (the room's decay and its noise; the room
    /// parameters are computed up to its end), e.g. `3s` for a hall; default: 1 s (or
    /// what the sweep needs), at most 20 s.
    #[arg(long, value_name = "TIME")]
    pub tail: Option<Time>,
    /// LF harmonics: `fine` gives finer low-frequency harmonics (sub and LF distortion), a
    /// higher floor there and a longer silence after the sweep.
    #[arg(long, value_enum, default_value_t = LfHarmonicsArg::Standard)]
    pub lf_harmonics: LfHarmonicsArg,
    /// Resolution of the stored run, points per octave: `1/12`, `1/24`, `1/48`, `1/96`.
    #[arg(long, value_name = "1/N", default_value = "1/48")]
    pub resolution: ResolutionArg,
    /// Name of the stored run (default: `Run <number>`).
    #[arg(long)]
    pub name: Option<String>,
    /// Take the stimulus lease over from another client.
    #[arg(long)]
    pub force: bool,
}

/// `sweep …`.
#[derive(Debug, Subcommand)]
pub enum SweepCmd {
    /// Run a sweep measurement in the foreground with its settings (Enter plays, Esc/Ctrl-C
    /// stops); the run is stored as a sweep trace under it, and a summary printed.
    Run(SweepRunArgs),
}

/// `sweep run`.
#[derive(Debug, Args)]
pub struct SweepRunArgs {
    /// The sweep measurement, by id or name.
    pub meas: MeasRef,
    /// Name of the stored run (default: `Run <number>`).
    #[arg(long)]
    pub name: Option<String>,
    /// Take the stimulus lease over from another client.
    #[arg(long)]
    pub force: bool,
}

/// Trace averaging method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AverageArg {
    /// RMS magnitude; phase of the complex mean.
    Power,
    /// Complex mean.
    Complex,
    /// Coherence-weighted complex mean.
    Coherence,
}

/// Import file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ImportFormatArg {
    /// A WAV impulse response when the file is WAV; ac2 CSV when it starts with its
    /// header; analyzer text otherwise.
    Auto,
    /// ac2 CSV only.
    Ac2,
    /// Analyzer text (REW, Smaart, … exports).
    Text,
    /// Impulse response, WAV / RF64.
    Wav,
}
