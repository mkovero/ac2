//! Command-line grammar.

use std::path::PathBuf;
use std::str::FromStr;

use ac2_client::RemoteAddr;
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::units::{
    Celsius, Channel, Channels, DelayAmount, Freq, LevelDbfs, SampleCount, SplLevel, Time,
};

/// ac2: live dual-channel analyzer — command-line client.
#[derive(Debug, Parser)]
#[command(name = "ac2", version = crate::BUILD_ID, about, propagate_version = true)]
pub struct Cli {
    /// Machine-readable JSON output (one document, or JSON lines for live views).
    #[arg(long, global = true)]
    pub json: bool,
    /// Connect to a remote daemon (CURVE; pin its key first with `ac2 auth pair`).
    #[arg(long, global = true, value_name = "HOST[:PORT]")]
    pub remote: Option<RemoteAddr>,
    /// Key directory (client keypair, pinned daemon keys).
    #[arg(long, global = true, value_name = "DIR")]
    pub key_dir: Option<PathBuf>,
    /// Reply deadline per try; each request is sent up to three times with the same id.
    #[arg(long, global = true, value_name = "TIME", default_value = "1.5s")]
    pub timeout: Time,
    /// Ctrl endpoint override (tests, unusual setups).
    #[arg(long, global = true, hide = true, requires = "data_endpoint")]
    pub ctrl_endpoint: Option<String>,
    /// Data endpoint override.
    #[arg(long, global = true, hide = true, requires = "ctrl_endpoint")]
    pub data_endpoint: Option<String>,
    /// Command.
    #[command(subcommand)]
    pub cmd: Cmd,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// List audio devices.
    Devices,
    /// Daemon status (same as `daemon status`).
    Status,
    /// Start, stop or inspect the local daemon.
    Daemon {
        /// Action.
        #[command(subcommand)]
        cmd: DaemonCmd,
    },
    /// Open or close the audio session.
    Session {
        /// Action.
        #[command(subcommand)]
        cmd: SessionCmd,
    },
    /// Generator: run a signal in the foreground (Enter fires, Esc/Ctrl-C stops), or stop.
    Gen {
        /// Signal or `stop`.
        #[command(subcommand)]
        cmd: GenCmd,
    },
    /// Measurements.
    Meas {
        /// Action.
        #[command(subcommand)]
        cmd: MeasCmd,
    },
    /// Delay finder and compensation of a transfer measurement.
    Delay {
        /// Action.
        #[command(subcommand)]
        cmd: DelayCmd,
    },
    /// SPL meter.
    Spl {
        /// Action.
        #[command(subcommand)]
        cmd: SplCmd,
    },
    /// Calibration.
    Cal {
        /// Action.
        #[command(subcommand)]
        cmd: CalCmd,
    },
    /// Loopback timing monitor.
    Timing {
        /// Live view.
        #[arg(long)]
        watch: bool,
    },
    /// Stored traces.
    Trace {
        /// Action.
        #[command(subcommand)]
        cmd: TraceCmd,
    },
    /// Daemon state.
    State {
        /// Action.
        #[command(subcommand)]
        cmd: StateCmd,
    },
    /// Remote-mode keys (client side).
    Auth {
        /// Action.
        #[command(subcommand)]
        cmd: AuthCmd,
    },
}

/// `daemon …`.
#[derive(Debug, Subcommand)]
pub enum DaemonCmd {
    /// Spawn the local `ac2d` (next to this binary, else on PATH) unless one is running.
    Start,
    /// Stop the local daemon (signals the pid in its pid file).
    Stop,
    /// Version, build id (stale check), incarnation, session.
    Status,
}

/// Audio backend, always named explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendArg {
    /// JACK.
    Jack,
    /// The OS audio host (ALSA, CoreAudio, WASAPI).
    Cpal,
    /// Simulated device; never touches hardware.
    Fake,
}

/// `session …`.
#[derive(Debug, Subcommand)]
pub enum SessionCmd {
    /// Open the session on one device of an explicitly named backend.
    Open(SessionOpen),
    /// Close the session.
    Close,
    /// Current session.
    Status,
}

/// `session open`.
#[derive(Debug, Args)]
pub struct SessionOpen {
    /// Backend (no default: `fake` only when asked for).
    #[arg(long, value_enum)]
    pub backend: BackendArg,
    /// Device id or name (default: the backend's only device).
    #[arg(long)]
    pub device: Option<String>,
    /// Input channels to capture, e.g. `1-4`.
    #[arg(long = "in", value_name = "CHANNELS")]
    pub inputs: Channels,
    /// Output channels of the stream.
    #[arg(long = "outputs", value_name = "N", default_value_t = 2)]
    pub outputs: u16,
    /// Sample rate, e.g. `48khz` (default: device default).
    #[arg(long)]
    pub rate: Option<Freq>,
    /// Buffer size, e.g. `256samples` (default: device default).
    #[arg(long)]
    pub buffer: Option<SampleCount>,
    /// Loopback output channel carrying the reference copy.
    #[arg(long, requires = "loopback_in")]
    pub loopback_out: Option<Channel>,
    /// Input channel the loopback returns on.
    #[arg(long, requires = "loopback_out")]
    pub loopback_in: Option<Channel>,
}

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

/// Kind of a new measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MeasKindArg {
    /// Dual-channel transfer function.
    Tf,
    /// Narrowband spectrum.
    Spectrum,
    /// Fractional-octave RTA.
    Rta,
    /// SPL meter.
    Spl,
}

/// Frequency weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WeightArg {
    /// A.
    A,
    /// C.
    C,
    /// Z (flat).
    Z,
}

/// SLM time weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TimeWeightArg {
    /// 125 ms.
    Fast,
    /// 1 s.
    Slow,
    /// Impulse.
    Impulse,
}

/// FFT window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WindowArg {
    /// Hann.
    Hann,
    /// 4-term Blackman-Harris.
    Bh4,
    /// Flat-top.
    Flattop,
    /// Rectangular.
    Rect,
}

/// Octave fraction 1/N.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FractionArg {
    /// 1/1.
    #[value(name = "1")]
    F1,
    /// 1/3.
    #[value(name = "3")]
    F3,
    /// 1/6.
    #[value(name = "6")]
    F6,
    /// 1/12.
    #[value(name = "12")]
    F12,
    /// 1/24.
    #[value(name = "24")]
    F24,
    /// 1/48 (smoothing only).
    #[value(name = "48")]
    F48,
}

/// A measurement by id or name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeasRef {
    /// Numeric id.
    Id(u32),
    /// Name.
    Name(String),
}

impl FromStr for MeasRef {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty measurement reference".into());
        }
        Ok(match s.parse::<u32>() {
            Ok(n) => Self::Id(n),
            Err(_) => Self::Name(s.to_owned()),
        })
    }
}

/// `meas …`.
#[derive(Debug, Subcommand)]
pub enum MeasCmd {
    /// Create a measurement.
    New(MeasNew),
    /// List measurements.
    List {
        /// Live view with data age and STALE.
        #[arg(long)]
        watch: bool,
    },
    /// Start a measurement.
    Start {
        /// Id or name.
        meas: MeasRef,
    },
    /// Stop a measurement.
    Stop {
        /// Id or name.
        meas: MeasRef,
    },
    /// Delete a measurement.
    Rm {
        /// Id or name.
        meas: MeasRef,
    },
}

/// `meas new`.
#[derive(Debug, Args)]
pub struct MeasNew {
    /// Kind.
    #[arg(value_enum)]
    pub kind: MeasKindArg,
    /// Name.
    #[arg(long)]
    pub name: String,
    /// Reference input (tf).
    #[arg(long = "ref")]
    pub reference: Option<Channel>,
    /// Measurement input (tf).
    #[arg(long = "meas")]
    pub measurement: Option<Channel>,
    /// Input (spectrum, rta, spl).
    #[arg(long)]
    pub input: Option<Channel>,
    /// Points per octave of the tf grid.
    #[arg(long, default_value_t = 48)]
    pub ppo: u32,
    /// tf live smoothing, 1/N octave.
    #[arg(long, value_enum)]
    pub smooth: Option<FractionArg>,
    /// tf averaging: FIFO blocks of the full-rate stage.
    #[arg(long, default_value_t = 8)]
    pub blocks: u32,
    /// Spectrum FFT length, samples.
    #[arg(long, default_value = "65536samples")]
    pub fft: SampleCount,
    /// Spectrum window.
    #[arg(long, value_enum, default_value = "hann")]
    pub window: WindowArg,
    /// RTA band fraction, 1/N octave.
    #[arg(long, value_enum, default_value = "3")]
    pub fraction: FractionArg,
    /// RTA lowest band.
    #[arg(long, default_value = "20hz")]
    pub from: Freq,
    /// RTA highest band.
    #[arg(long, default_value = "20khz")]
    pub to: Freq,
    /// Frequency weighting (rta, spl).
    #[arg(long, value_enum, default_value = "z")]
    pub weight: WeightArg,
    /// Time weighting (spl).
    #[arg(long, value_enum, default_value = "fast")]
    pub time: TimeWeightArg,
    /// Start right away.
    #[arg(long)]
    pub start: bool,
}

/// Which finder result to insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickArg {
    /// First arrival (the default).
    First,
    /// Strongest peak.
    Strongest,
    /// Candidate index (0 = strongest).
    Candidate(u8),
}

impl FromStr for PickArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "first" | "first-arrival" => Ok(Self::First),
            "strongest" => Ok(Self::Strongest),
            n => n
                .parse::<u8>()
                .map(Self::Candidate)
                .map_err(|_| format!("{s:?}: expected first, strongest or a candidate index")),
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

/// `delay …`.
#[derive(Debug, Subcommand)]
pub enum DelayCmd {
    /// Run the delay finder.
    Find {
        /// Transfer measurement.
        meas: MeasRef,
        /// Insert a result right away (default: first arrival).
        #[arg(long, num_args = 0..=1, default_missing_value = "first", value_name = "PICK")]
        insert: Option<PickArg>,
    },
    /// Insert the last finder result.
    Insert {
        /// Transfer measurement.
        meas: MeasRef,
        /// first, strongest or a candidate index.
        #[arg(long, default_value = "first")]
        pick: PickArg,
    },
    /// Set the delay: `12.5ms`, `600samples` or a distance `4.3m`.
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
    /// Track the delay continuously.
    Track {
        /// Transfer measurement.
        meas: MeasRef,
        /// on or off.
        #[arg(value_enum)]
        state: Switch,
    },
}

/// `spl …`.
#[derive(Debug, Subcommand)]
pub enum SplCmd {
    /// Live SPL readout (q/Esc/Ctrl-C quits). Uses `--meas`, or an SPL measurement on
    /// `--input` (created for the duration of the command if none exists).
    Watch(SplWatch),
    /// Calibrate an input against an acoustic calibrator (same as `cal spl`).
    Cal(CalSpl),
}

/// `spl watch`.
#[derive(Debug, Args)]
pub struct SplWatch {
    /// Existing SPL measurement.
    #[arg(long, conflicts_with = "input")]
    pub meas: Option<MeasRef>,
    /// Input channel.
    #[arg(long, required_unless_present = "meas")]
    pub input: Option<Channel>,
    /// Frequency weighting.
    #[arg(long, value_enum, default_value = "a")]
    pub weight: WeightArg,
    /// Time weighting.
    #[arg(long, value_enum, default_value = "fast")]
    pub time: TimeWeightArg,
}

/// `cal …`.
#[derive(Debug, Subcommand)]
pub enum CalCmd {
    /// Calibrate an input against an acoustic calibrator.
    Spl(CalSpl),
    /// List calibrations.
    List,
}

/// `cal spl`.
#[derive(Debug, Args)]
pub struct CalSpl {
    /// Input channel with the calibrator on its mic.
    #[arg(long)]
    pub input: Channel,
    /// Calibrator level, e.g. `94db`.
    #[arg(long = "ref", value_name = "SPL")]
    pub reference: SplLevel,
    /// Calibrator frequency.
    #[arg(long, default_value = "1khz")]
    pub freq: Freq,
    /// Mic name the calibration is bound to.
    #[arg(long, default_value = "mic")]
    pub mic: String,
}

/// `trace …`.
#[derive(Debug, Subcommand)]
pub enum TraceCmd {
    /// Capture a measurement's live result.
    Capture {
        /// Source measurement.
        meas: MeasRef,
        /// Trace name.
        #[arg(long)]
        name: String,
    },
    /// List traces.
    List,
    /// Export a trace.
    Export {
        /// Trace id or name.
        trace: MeasRef,
        /// Write ac2 CSV to this file (`-` = stdout).
        #[arg(long, value_name = "FILE")]
        csv: PathBuf,
    },
}

/// `state …`.
#[derive(Debug, Subcommand)]
pub enum StateCmd {
    /// Print the full state snapshot as JSON.
    Dump,
}

/// `auth …`.
#[derive(Debug, Subcommand)]
pub enum AuthCmd {
    /// Pin a daemon's public key and print this client's key for the daemon operator to
    /// authorize. Compare the printed fingerprint with the one the daemon shows.
    Pair {
        /// Daemon host as used with `--remote`.
        host: RemoteAddr,
        /// The daemon's Z85 public key (40 characters).
        #[arg(long)]
        server_key: String,
        /// Name to authorize this client under (default: this host's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Show this client's key and the pinned daemon keys.
    Show,
}
