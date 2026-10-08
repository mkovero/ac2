//! Command-line grammar.

use std::path::PathBuf;
use std::str::FromStr;

use ac2_client::RemoteAddr;
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::units::{
    BandLimitArg, BandSourceArg, BandWindowArg, ByteSize, Celsius, Channel, Channels, DayOffsetArg,
    DelayAmount, Freq, Gain, LeqLimitArg, LeqWindowArg, LevelDbfs, MicSensitivityArg, PeakLimitArg,
    PositionArg, SampleCount, SplLevel, Time, TimeRef, VoltsArg,
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
    /// Math channels: live results computed from measurements and stored traces by name
    /// (`A ÷ B`, `A × B`, `A + B`, `A − B`, the average of several).
    Math {
        /// Action.
        #[command(subcommand)]
        cmd: MathCmd,
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
    /// Run a sweep measurement (`meas new sweep`) with its settings: each run is stored as
    /// a sweep trace under it.
    Sweep {
        /// Action.
        #[command(subcommand)]
        cmd: SweepCmd,
    },
    /// Sweep measurement: response, harmonic distortion and impulse response.
    Ir {
        /// Action.
        #[command(subcommand)]
        cmd: IrCmd,
    },
    /// Raw capture files: record inputs of the open session losslessly, with a sidecar of
    /// what was measured; play one back with `session replay`.
    Rec {
        /// Action.
        #[command(subcommand)]
        cmd: RecCmd,
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
    /// List ac2 daemons on the local network (mDNS).
    ///
    /// Listing a rig does not make it trusted: connect with `--remote` only after
    /// `ac2 auth pair`, which pins the daemon key you verified on the daemon host.
    Discover(DiscoverArgs),
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
    /// JACK (JACK2, or PipeWire through pipewire-jack): the real backend on Linux.
    Jack,
    /// The OS audio host (Core Audio, WASAPI): the real backend on macOS and Windows.
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
    /// Save measurements and traces (with slots and display edits) to a session.
    Save {
        /// Session name (in the daemon's session directory) or a directory path.
        session: String,
    },
    /// Load a session: replaces measurements and traces; always comes up disarmed.
    Load {
        /// Session name or directory path.
        session: String,
    },
    /// Saved sessions in the daemon's session directory.
    List,
    /// Open a session that plays a recording (`ac2 rec list`) instead of a device: the
    /// running measurements analyse it as they did the live inputs.
    Replay {
        /// Recording name (in the daemon's recording directory) or the path of its .wav or
        /// .ac2rec.json file.
        recording: String,
        /// As fast as the measurements take it, instead of in real time.
        #[arg(long)]
        fast: bool,
    },
    /// Input setup: the mic on each input and its active mic curve (shown without options).
    Inputs(SessionInputs),
}

/// `IN=NAME`: the mic on a 1-based input; `IN=` clears the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicAssign {
    /// Input.
    pub input: Channel,
    /// Mic name; `None` clears it.
    pub mic: Option<String>,
}

impl FromStr for MicAssign {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (ch, name) = s
            .split_once('=')
            .ok_or_else(|| format!("{s:?}: expected IN=NAME, e.g. 3=M30"))?;
        let input: Channel = ch.parse().map_err(|e: crate::units::UnitError| e.0)?;
        let name = name.trim();
        Ok(Self {
            input,
            mic: (!name.is_empty()).then(|| name.to_owned()),
        })
    }
}

/// A curve choice: a label of the mic's curves, or `off` / `none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurveArg(pub ac2_proto::model::CurveChoice);

impl FromStr for CurveArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        use ac2_proto::model::CurveChoice;
        let v = s.trim();
        if v.is_empty() {
            return Err("expected a curve label (e.g. 90°) or off".into());
        }
        Ok(Self(
            if v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("none") {
                CurveChoice::Off
            } else {
                CurveChoice::Curve {
                    label: v.to_owned(),
                }
            },
        ))
    }
}

/// `IN=LABEL|off`: the active mic curve of a 1-based input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurveAssign {
    /// Input.
    pub input: Channel,
    /// The curve.
    pub curve: CurveArg,
}

impl FromStr for CurveAssign {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("{s:?}: expected IN=LABEL or IN=off, e.g. 3=90°");
        let (ch, v) = s.split_once('=').ok_or_else(bad)?;
        let input: Channel = ch.parse().map_err(|e: crate::units::UnitError| e.0)?;
        Ok(Self {
            input,
            curve: v.parse()?,
        })
    }
}

/// `session inputs`.
#[derive(Debug, Args)]
pub struct SessionInputs {
    /// Mic on an input, e.g. `3=M30` (repeatable; `3=` clears the name).
    #[arg(long = "mic", value_name = "IN=NAME")]
    pub mics: Vec<MicAssign>,
    /// Active mic curve of an input, e.g. `3=90°` or `3=off` (repeatable).
    #[arg(long = "curve", value_name = "IN=LABEL|off")]
    pub curves: Vec<CurveAssign>,
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
    /// Mic on an input, e.g. `3=M30` (repeatable): the input setup calibrations match on.
    #[arg(long = "mic", value_name = "IN=NAME")]
    pub mics: Vec<MicAssign>,
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
    /// Sweep measurement: settings only (`--ref`, `--meas`, `--out`, `--level`, …); `ac2
    /// sweep run` plays it.
    Sweep,
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

/// A measurement (or trace) by id or name, as typed. Which one it means is decided against
/// the daemon's state: a name may be all digits, so `1083` can be an id, a name, or both
/// (then it is refused as ambiguous).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasRef(pub String);

impl MeasRef {
    /// The id it would be, if it is a number.
    pub fn id(&self) -> Option<u32> {
        self.0.parse().ok()
    }
}

impl FromStr for MeasRef {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty measurement reference".into());
        }
        Ok(Self(s.to_owned()))
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
    /// Delete a measurement. One that owns stored traces or math channels needs
    /// `--keep-traces` (they move to the imported group) or `--delete-traces`.
    Rm {
        /// Id or name.
        meas: MeasRef,
        /// Keep its traces and math channels, moved to the imported group.
        #[arg(long, conflicts_with = "delete_traces")]
        keep_traces: bool,
        /// Delete its traces and math channels with it.
        #[arg(long)]
        delete_traces: bool,
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
    /// Reference input (tf, sweep).
    #[arg(long = "ref")]
    pub reference: Option<Channel>,
    /// Measurement input (tf, sweep).
    #[arg(long = "meas")]
    pub measurement: Option<Channel>,
    /// Sweep: outputs playing it, the speaker's and the loopback's, e.g. `1,2`.
    #[arg(long = "out", value_name = "OUTS")]
    pub outputs: Option<Channels>,
    /// Sweep: RMS level, e.g. `-50dbfs`; required for a sweep (there is no default level).
    #[arg(long, allow_hyphen_values = true)]
    pub level: Option<LevelDbfs>,
    /// Sweep: duration (default 3 s).
    #[arg(long, value_name = "TIME")]
    pub duration: Option<Time>,
    /// Sweep: sweeps played and averaged per run (1 … 8, default 1).
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=8))]
    pub repeats: Option<u8>,
    /// Sweep: gate the linear response this long after the arrival.
    #[arg(long, value_name = "TIME")]
    pub gate: Option<Time>,
    /// Sweep: silence recorded after each sweep (at most 20 s).
    #[arg(long, value_name = "TIME")]
    pub tail: Option<Time>,
    /// Input (spectrum, rta, spl).
    #[arg(long)]
    pub input: Option<Channel>,
    /// Points per octave of the tf grid.
    #[arg(long, default_value_t = ac2_proto::model::TransferConfig::DEFAULT_PPO)]
    pub ppo: u32,
    /// Display smoothing, 1/N octave (tf: magnitude and phase; spectrum: power, and the
    /// level then no longer reads as tone level).
    #[arg(long, value_enum)]
    pub smooth: Option<FractionArg>,
    /// tf: smooth the magnitude only and keep the measured phase.
    #[arg(long, requires = "smooth")]
    pub smooth_magnitude_only: bool,
    /// tf averaging: FIFO blocks of the full-rate stage.
    #[arg(long, default_value_t = ac2_proto::model::TransferConfig::DEFAULT_BLOCKS)]
    pub blocks: u32,
    /// tf: cap the low-frequency stages' averaging span (default 1s) for faster settling,
    /// at a higher coherence floor there. Default: equal confidence at every frequency.
    #[arg(long, num_args = 0..=1, default_missing_value = "1s", value_name = "TIME")]
    pub fast_lf: Option<Time>,
    /// Spectrum FFT length, samples.
    #[arg(long, default_value = "65536samples")]
    pub fft: SampleCount,
    /// Spectrum window.
    #[arg(long, value_enum, default_value = "hann")]
    pub window: WindowArg,
    /// RTA band fraction, 1/N octave.
    #[arg(long, value_enum, default_value = "3")]
    pub fraction: FractionArg,
    /// RTA lowest band; sweep start frequency.
    #[arg(long, default_value = "20hz")]
    pub from: Freq,
    /// RTA highest band; sweep end frequency.
    #[arg(long, default_value = "20khz")]
    pub to: Freq,
    /// Frequency weighting (rta: default z; spl: default a).
    #[arg(long, value_enum)]
    pub weight: Option<WeightArg>,
    /// Time weighting (spl).
    #[arg(long, value_enum, default_value = "fast")]
    pub time: TimeWeightArg,
    /// Start right away.
    #[arg(long)]
    pub start: bool,
}

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
                let lo: Freq = lo.parse().map_err(|e: crate::units::UnitError| e.0)?;
                let hi: Freq = hi.parse().map_err(|e: crate::units::UnitError| e.0)?;
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

/// `spl …`.
#[derive(Debug, Subcommand)]
pub enum SplCmd {
    /// Live SPL readout (q/Esc/Ctrl-C quits). Uses `--meas`, or its own SPL meter on
    /// `--input`, created for the duration of the command (Leq, Lmax and Lmin integrate
    /// from the command's start).
    Watch(SplWatch),
    /// Calibrate an input against an acoustic calibrator (same as `cal spl`).
    Cal(CalSpl),
    /// Rolling Leq windows of an SPL meter: watch them, set windows and limits, export the
    /// per-second log.
    Leq {
        #[command(subcommand)]
        cmd: LeqCmd,
    },
    /// Set a running meter's frequency and time weighting (`--weight c --time slow`): it
    /// carries on, its Leq windows and log untouched.
    Set(SplSet),
    /// The band meter of an SPL meter: 1/3-octave band Leq over windows of their own length
    /// and weighting against per-band limits, judged at the mic as typed or moved there from
    /// a named place by a measured band transfer, and the predicted LAeq at that place.
    /// Informational, not legal advice.
    Bands {
        #[command(subcommand)]
        cmd: BandsCmd,
    },
}

/// `spl bands …`.
#[derive(Debug, Subcommand)]
pub enum BandsCmd {
    /// Each band window's shown bands, the worst band and what to do about it, the
    /// predicted LAeq (q/Esc/Ctrl-C quits). With `--json` one line per second, the windows
    /// in an array.
    Watch(LeqWatch),
    /// Turn the band meter on, set its windows, bands, limits and §13 corrections (a
    /// preset sets windows, limits and bands at once), or turn it off; the meter, its log
    /// and a measured transfer carry on.
    Set(BandsSet),
    /// Measure the band transfer FOH → a place (`--place`, `receiving room` unless named)
    /// from band levels at FOH and at the place of the same steady test signal, and the
    /// place's background with the system silent; stored in the meter's band meter.
    Transfer(BandsTransfer),
    /// A span of the meter's band log: the energy average per band (as `transfer` takes
    /// it from `METER@FROM..UNTIL`) and how much of the span was logged; `--step` adds the
    /// logged seconds. `--levels-out` writes the averages as a `<Hz> <dB>` file that
    /// `transfer --place-levels FILE` (or `--foh`, `--background`) reads on another rig.
    Log(BandsLog),
    /// Store an estimated transfer where the place cannot be reached: per-band
    /// attenuation typed from a `<Hz> <dB>` file, marked estimated everywhere it shows.
    Estimate(BandsEstimate),
}

/// `spl bands estimate`.
#[derive(Debug, Args)]
pub struct BandsEstimate {
    #[command(flatten)]
    pub meter: MeterRef,
    /// `<Hz> <dB>` lines: each band's estimated attenuation FOH → the place (dB, ≥ 0); a
    /// band not in the file has no limit at FOH.
    #[arg(long, value_name = "FILE")]
    pub attenuation: std::path::PathBuf,
    /// The place the limits are for, as every line naming it says: `flat 4 bedroom`.
    #[arg(long, value_name = "NAME", default_value = ac2_proto::model::BandTransferSet::DEFAULT_PLACE)]
    pub place: String,
}

/// Band meter presets (not legal advice; `docs/design/band-leq.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BandPresetArg {
    /// Finland, STM 545/2015 Liite 2 Taulukko 2, rooms meant for sleeping: LZeq 60 min,
    /// 20 … 200 Hz, night 74 … 32 dB, day 5 dB higher; predicted LAeq 60 min at night
    /// ≤ 25 dB.
    #[value(name = "finland-545-lf")]
    Finland545Lf,
    /// Finland, STM 545/2015 Liite 2 Taulukko 1, living rooms: LZeq 60 min, 20 … 200 Hz
    /// without band limits; predicted LAeq 60 min day ≤ 35 dB, night ≤ 30 dB.
    #[value(name = "finland-545-living-room")]
    Finland545LivingRoom,
}

/// §13 impulse correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ImpulseArg {
    None,
    /// +5 dB.
    #[value(name = "5")]
    Plus5,
    /// +10 dB.
    #[value(name = "10")]
    Plus10,
}

/// §13 narrowband (tonal) correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TonalArg {
    None,
    /// +3 dB.
    #[value(name = "3")]
    Plus3,
    /// +6 dB.
    #[value(name = "6")]
    Plus6,
}

/// `spl bands set`.
#[derive(Debug, Args)]
#[command(group(clap::ArgGroup::new("what").required(true).multiple(true)
    .args(["preset", "windows", "limits", "day_offsets", "impulse", "tonal", "warn",
        "off"])))]
pub struct BandsSet {
    #[command(flatten)]
    pub meter: MeterRef,
    /// A rule's windows, limits, bands and predicted window (the corrections and a
    /// transfer are kept); everything stays settable after. Informational, not legal
    /// advice.
    #[arg(long, value_enum)]
    pub preset: Option<BandPresetArg>,
    /// The band windows, replacing the meter's, each its length and weighting and its bands
    /// after `@`: `z:1min@20hz,z:60min@20hz..200hz` (Z unless `a:` or `c:`; any Leq length
    /// up to 24 h; one band or a range; 20 … 200 Hz without `@`). A window kept (same bands,
    /// length and weighting) keeps its limits; at most 8.
    #[arg(long, value_delimiter = ',', value_name = "WINDOWS")]
    pub windows: Option<Vec<BandWindowArg>>,
    /// A band limit of a window, dB SPL where the limits apply: `z:60min:63hz=42db`,
    /// `z:60min@20hz..200hz:63hz=42db`, a single-band window's `z:1min@20hz=80db` (name the
    /// window's bands when two windows share length and weighting); `…=none` removes it;
    /// repeatable.
    #[arg(long = "limit", value_name = "WINDOW:BAND=LIMIT")]
    pub limits: Vec<BandLimitArg>,
    /// `z:60min=5db` (`z:1min@20hz=5db`): the window's limits hold at night (22–07), the
    /// day's this much higher; `z:60min=none`: one set day and night; repeatable.
    #[arg(long = "day-offset", value_name = "WINDOW=DB")]
    pub day_offsets: Vec<DayOffsetArg>,
    /// §13 impulse correction in force from now: `none`, `5` or `10` dB.
    #[arg(long, value_enum)]
    pub impulse: Option<ImpulseArg>,
    /// §13 narrowband correction in force from now: `none`, `3` or `6` dB.
    #[arg(long, value_enum)]
    pub tonal: Option<TonalArg>,
    /// Warn margin of every band window: near when this close below a limit, e.g. `3db`.
    #[arg(long, value_name = "DB")]
    pub warn: Option<Gain>,
    /// Turn the band meter off (its configuration and transfer go).
    #[arg(long, conflicts_with_all = ["preset", "windows", "limits", "day_offsets",
        "impulse", "tonal", "warn"])]
    pub off: bool,
}

/// `spl bands transfer`. Each source is a file of `<Hz> <dB>` lines (dB SPL per 1/3-octave
/// band, from another instrument), or a span of an SPL meter's band log:
/// `METER@FROM..UNTIL`, the meter by name or id, the times `21:00:30` (local, today),
/// `2026-10-08T21:00:30` (local), `…Z` (UTC), `-30s` (before now) or `now`.
#[derive(Debug, Args)]
pub struct BandsTransfer {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Band levels at FOH (the meter's mic) with the test signal playing.
    #[arg(long, value_name = "SOURCE")]
    pub foh: BandSourceArg,
    /// Band levels at the place, the same signal at the same level.
    #[arg(long = "place-levels", value_name = "SOURCE")]
    pub at_place: BandSourceArg,
    /// The place's background with the system silent; without it every band is
    /// unchecked.
    #[arg(long, value_name = "SOURCE")]
    pub background: Option<BandSourceArg>,
    /// The place the limits are for, as every line naming it says: `flat 4 bedroom`.
    #[arg(long, value_name = "NAME", default_value = ac2_proto::model::BandTransferSet::DEFAULT_PLACE)]
    pub place: String,
}

/// `spl bands log`. Times: `21:00:30` (local, today), `2026-10-08T21:00:30` (local),
/// `…Z` (UTC), `-30s` (before now) or `now`.
#[derive(Debug, Args)]
pub struct BandsLog {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Start of the span.
    #[arg(long, value_name = "TIME", allow_hyphen_values = true)]
    pub from: TimeRef,
    /// End of the span.
    #[arg(
        long,
        value_name = "TIME",
        default_value = "now",
        allow_hyphen_values = true
    )]
    pub until: TimeRef,
    /// Also every STEP-th logged second (1: each; at most 3600 rows).
    #[arg(long, value_name = "STEP")]
    pub step: Option<u32>,
    /// Write the span's per-band averages (dB SPL) to FILE as `<Hz> <dB>` lines.
    #[arg(long, value_name = "FILE")]
    pub levels_out: Option<std::path::PathBuf>,
}

/// `spl set`.
#[derive(Debug, Args)]
#[command(group(clap::ArgGroup::new("what").required(true).multiple(true).args(["weight", "time"])))]
pub struct SplSet {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Frequency weighting.
    #[arg(long, value_enum)]
    pub weight: Option<WeightArg>,
    /// Time weighting.
    #[arg(long, value_enum)]
    pub time: Option<TimeWeightArg>,
}

/// `spl leq …`.
#[derive(Debug, Subcommand)]
pub enum LeqCmd {
    /// Big-number view of the meter's windows: value, limit, state, headroom (q/Esc/Ctrl-C
    /// quits). With `--json` one line per second.
    Watch(LeqWatch),
    /// Set the meter's windows, limits, warn margin, a preset or the headroom horizon; the
    /// meter, its log and its windows carry on.
    Set(LeqSet),
    /// Write the meter's per-second log (LAeq, LCeq, LZeq per second) as CSV.
    Export(LeqExport),
    /// End the meter's log and start a new one (show start after a loud soundcheck): the
    /// windows, their states, the alarms, the run clock and the total start over; windows
    /// and limits are kept. Asks for `--yes`; `--export FILE` writes the ended log first.
    New(LeqNew),
}

/// Which SPL meter: `--meas`, the one on `--input`, or the only one.
#[derive(Debug, Args)]
pub struct MeterRef {
    /// SPL measurement (id or name).
    #[arg(long, conflicts_with = "input")]
    pub meas: Option<MeasRef>,
    /// The SPL meter on this input.
    #[arg(long)]
    pub input: Option<Channel>,
}

/// `spl leq watch`.
#[derive(Debug, Args)]
pub struct LeqWatch {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Quit after this long, e.g. `10s` (scripts).
    #[arg(long = "for", value_name = "TIME")]
    pub duration: Option<Time>,
}

/// Informational limit presets (not legal advice; `docs/design/leq.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PresetArg {
    /// DIN 15905-5: LAeq 30 min ≤ 99 dB.
    #[value(name = "din15905")]
    Din15905,
    /// Swiss V-NISSG: LAeq 60 min ≤ 93 dB.
    #[value(name = "swiss93")]
    Swiss93,
    /// Swiss V-NISSG: LAeq 60 min ≤ 96 dB.
    #[value(name = "swiss96")]
    Swiss96,
    /// Swiss V-NISSG: LAeq 60 min ≤ 100 dB.
    #[value(name = "swiss100")]
    Swiss100,
    /// WHO safe listening (2022): LAeq 15 min ≤ 100 dB.
    Who,
    /// France, Code de la santé publique art. R1336-1: LAeq 15 min ≤ 102 dB, LCeq 15 min
    /// ≤ 118 dB.
    #[value(name = "france")]
    France,
    /// France, art. R1336-1, events for children up to six: LAeq 15 min ≤ 94 dB, LCeq
    /// 15 min ≤ 104 dB.
    #[value(name = "france-children")]
    FranceChildren,
    /// Flanders, VLAREM II art. 6.7.3: LAeq 15 min ≤ 85 dB.
    #[value(name = "flanders-85")]
    Flanders85,
    /// Flanders, VLAREM II art. 5.32.2.2bis § 1: LAeq 15 min ≤ 95 dB.
    #[value(name = "flanders-95")]
    Flanders95,
    /// Flanders, VLAREM II art. 5.32.2.2bis § 2: LAeq 60 min ≤ 100 dB, LAeq 15 min shown.
    #[value(name = "flanders-100")]
    Flanders100,
    /// Brussels-Capital, arrêté of 26 January 2017 art. 3: LAeq 15 min ≤ 85 dB.
    #[value(name = "brussels-85")]
    Brussels85,
    /// Brussels-Capital, art. 4: LAeq 15 min ≤ 95 dB, LCeq 15 min ≤ 110 dB.
    #[value(name = "brussels-95")]
    Brussels95,
    /// Brussels-Capital, art. 5: LAeq 60 min ≤ 100 dB, LCeq 60 min ≤ 115 dB.
    #[value(name = "brussels-100")]
    Brussels100,
    /// Netherlands, fourth covenant (voluntary) art. 3.1.2: LAeq 15 min ≤ 103 dB.
    #[value(name = "nl-covenant")]
    NlCovenant,
    /// Netherlands covenant art. 3.1.3, audiences of 16–17: LAeq 15 min ≤ 100 dB.
    #[value(name = "nl-covenant-16-17")]
    NlCovenant16To17,
    /// Netherlands covenant art. 3.1.3, audiences of 14–15: LAeq 15 min ≤ 96 dB.
    #[value(name = "nl-covenant-14-15")]
    NlCovenant14To15,
    /// Netherlands covenant art. 3.1.3, audiences up to 13: LAeq 15 min ≤ 91 dB.
    #[value(name = "nl-covenant-13")]
    NlCovenantTo13,
    /// Finland, STM 545/2015 §12: LAeq 4 h ≤ 100 dB, LAFmax ≤ 115 dB, LCpeak ≤ 140 dB.
    #[value(name = "finland-545")]
    Finland545,
}

/// `spl leq set`.
#[derive(Debug, Args)]
pub struct LeqSet {
    #[command(flatten)]
    pub meter: MeterRef,
    /// The windows, replacing the meter's: `1min,5min,10min,30min,60min` (A-weighted;
    /// `c:30s` for C). Windows kept keep their limits. With `--preset`: windows added to
    /// the preset's, without limits.
    #[arg(long, value_delimiter = ',', value_name = "WINDOWS")]
    pub windows: Option<Vec<LeqWindowArg>>,
    /// Replace the windows with exactly a preset's, with its limits (the meter's other
    /// windows and limits go); repeatable: the windows of all, a window two share at the
    /// lower limit. Informational, not legal advice.
    #[arg(long, value_enum)]
    pub preset: Vec<PresetArg>,
    /// A window's limit: `30min=99db`, `30min=none` removes it; repeatable.
    #[arg(long = "limit", value_name = "WINDOW=LIMIT")]
    pub limits: Vec<LeqLimitArg>,
    /// Warn margin of every window: near when this close below the limit, e.g. `3db`.
    #[arg(long, value_name = "DB")]
    pub warn: Option<Gain>,
    /// Headroom horizon: the steady level allowed over this much of the future, e.g. `1min`.
    #[arg(long, value_name = "WINDOW")]
    pub horizon: Option<LeqWindowArg>,
    /// A peak limit, judged on the highest second of the last 10 s: `lcpeak=135db`,
    /// `lafmax=125db`, `lcpeak=none` removes it; repeatable. A preset sets its rule's.
    #[arg(long = "peak-limit", value_name = "QUANTITY=LIMIT")]
    pub peak_limits: Vec<PeakLimitArg>,
    /// Measuring-position correction added to every level the meter reports (and judges)
    /// once calibrated: the difference from the mic to where the limit applies, e.g. `4db`
    /// (FOH → loudest audience spot); `none` removes it. The log keeps what was measured.
    #[arg(long, value_name = "DB")]
    pub position: Option<PositionArg>,
    /// The correction of the peak levels (LCpeak), when it differs from `--position`'s.
    #[arg(long, value_name = "DB")]
    pub position_peak: Option<Gain>,
}

/// `spl leq export`.
#[derive(Debug, Args)]
pub struct LeqExport {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Write to this file (default: standard output).
    #[arg(long, short = 'o', value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// The log `spl leq new` ended last (kept until the next new log or a daemon restart).
    #[arg(long)]
    pub previous: bool,
}

/// `spl leq new`.
#[derive(Debug, Args)]
pub struct LeqNew {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Write the ended log to this file as CSV (as `spl leq export`), all of it.
    #[arg(long, value_name = "FILE")]
    pub export: Option<PathBuf>,
    /// Go ahead: the current log's windows, alarms, run clock and total are discarded.
    #[arg(long)]
    pub yes: bool,
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
    /// Quit after this long, e.g. `10s` (scripts).
    #[arg(long = "for", value_name = "TIME")]
    pub duration: Option<Time>,
}

/// `cal …`.
#[derive(Debug, Subcommand)]
pub enum CalCmd {
    /// Calibrate an input against an acoustic calibrator.
    Spl(CalSpl),
    /// Calibrate an input without a calibrator: a voltage measured at the input with a
    /// meter (DMM, Analog Discovery) while ac2 reads its level, and the mic's sensitivity.
    ///
    /// In-line (default): the mic stays connected and powered and hears a steady tone
    /// (1 kHz); measure AC volts between XLR pins 2 and 3 with a breakout. Phantom power is
    /// +48 V on pins 2 and 3 against pin 1: measure pins 2–3 only, never to pin 1, never
    /// short pins. Injected: a generator in place of the mic — switch phantom power OFF on
    /// that input first (48 V can damage the generator; ac2 cannot switch it) and back on
    /// for the mic afterwards. Either way keep the gain you will measure with.
    Electrical(CalElectrical),
    /// The mic library: import, rename and delete a mic's curves.
    #[command(subcommand)]
    Curve(CalCurveCmd),
    /// Choose the mic curve an input applies: a label of its mic's curves, or `off`.
    Use {
        /// Input channel.
        input: Channel,
        /// Curve label (e.g. 90°), or `off`.
        curve: CurveArg,
    },
    /// List sensitivity calibrations, the mic library and what each input uses.
    List,
    /// Delete a sensitivity calibration.
    Rm(CalRm),
}

/// `cal curve …`.
#[derive(Debug, Subcommand)]
pub enum CalCurveCmd {
    /// Import a mic's magnitude curve (.frd / .txt / CSV) into the mic library. The label
    /// defaults to the angle the file names (`90°`), else its file name. With `--input`, the
    /// mic becomes that input's mic, and its only curve its active one.
    Import(CalCurveImport),
    /// Rename a curve (inputs that use it follow).
    Rename {
        /// Mic name.
        #[arg(long)]
        mic: String,
        /// Current label.
        label: String,
        /// New label.
        new_label: String,
    },
    /// Delete a curve from the mic library.
    Rm {
        /// Mic name.
        #[arg(long)]
        mic: String,
        /// Label.
        label: String,
    },
}

/// `cal curve import`.
#[derive(Debug, Args)]
pub struct CalCurveImport {
    /// Curve file: frequency and dB per line (further columns ignored).
    pub file: PathBuf,
    /// Mic name (default: the mic on `--input`).
    #[arg(long, required_unless_present = "input")]
    pub mic: Option<String>,
    /// Label among the mic's curves, e.g. `90°` (default: from the file).
    #[arg(long)]
    pub label: Option<String>,
    /// Input the mic is on: sets its mic name.
    #[arg(long)]
    pub input: Option<Channel>,
}

/// `cal rm`.
#[derive(Debug, Args)]
pub struct CalRm {
    /// Input channel of the calibration.
    #[arg(long)]
    pub input: Channel,
    /// Mic name of the calibration (default: the input's mic name in the session's input
    /// setup).
    #[arg(long)]
    pub mic: Option<String>,
    /// Capture device of the calibration (default: the open session's; without a session,
    /// the one device holding a calibration for this input and mic).
    #[arg(long, value_name = "ID")]
    pub device: Option<String>,
}

/// Where the voltage of an electrical calibration is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ElectricalMethodArg {
    /// Pins 2–3 with the mic connected and powered, a tone at the mic.
    Inline,
    /// A generator in place of the mic, phantom power off.
    Injected,
}

/// `cal electrical`.
#[derive(Debug, Args)]
pub struct CalElectrical {
    /// Input channel.
    #[arg(long)]
    pub input: Channel,
    /// Voltage measured at the input, RMS, e.g. `15.03mv`.
    #[arg(long, value_name = "VOLTS")]
    pub volts: VoltsArg,
    /// Frequency of the tone (mic sensitivities are stated at 1 kHz).
    #[arg(long, default_value = "1khz")]
    pub freq: Freq,
    /// Mic sensitivity, e.g. `15.0mv/pa` or `-36.5dbv/pa` (default: the data-sheet value the
    /// mic's curve files state, when they state one).
    #[arg(long, value_name = "MV/PA", allow_hyphen_values = true)]
    pub sensitivity: Option<MicSensitivityArg>,
    /// Where the voltage is measured.
    #[arg(long, value_enum, default_value = "inline")]
    pub method: ElectricalMethodArg,
    /// Stated uncertainty, e.g. `0.5db` (default ±1 dB: the data-sheet tolerance).
    #[arg(long, value_name = "DB")]
    pub uncertainty: Option<Gain>,
    /// Mic name the calibration is bound to (default: the input's mic name in the
    /// session's input setup); it also becomes the input's mic name.
    #[arg(long)]
    pub mic: Option<String>,
    /// Replace an acoustic calibration of this input and mic (a calibrator reading is the
    /// better one; refused otherwise).
    #[arg(long)]
    pub replace_acoustic: bool,
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
    /// Mic name the calibration is bound to (default: the input's mic name in the
    /// session's input setup); it also becomes the input's mic name.
    #[arg(long)]
    pub mic: Option<String>,
}

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
    /// Import a CSV / analyzer text export (freq, mag[, phase][, coherence]).
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
    },
    /// Export a trace.
    Export {
        /// Trace id or name.
        trace: MeasRef,
        /// Write ac2 CSV to this file (`-` = stdout).
        #[arg(long, value_name = "FILE")]
        csv: PathBuf,
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
    /// ac2 CSV when the file starts with its header, analyzer text otherwise.
    Auto,
    /// ac2 CSV only.
    Ac2,
    /// Analyzer text (REW, Smaart, … exports).
    Text,
}

/// `state …`.
#[derive(Debug, Subcommand)]
pub enum StateCmd {
    /// Print the full state snapshot as JSON.
    Dump,
}

/// `discover`.
#[derive(Debug, Args)]
pub struct DiscoverArgs {
    /// How long to listen for adverts.
    #[arg(long, value_name = "TIME", default_value = "2s")]
    pub wait: Time,
    /// mDNS port (tests).
    #[arg(long, hide = true)]
    pub mdns_port: Option<u16>,
    /// Browse on 127.0.0.1 only (tests).
    #[arg(long, hide = true)]
    pub loopback: bool,
}

/// `auth …`.
#[derive(Debug, Subcommand)]
pub enum AuthCmd {
    /// Pin a daemon's public key and print this client's key for the daemon operator to
    /// authorize. Compare the printed fingerprint with the one the daemon shows.
    Pair {
        /// Daemon host as used with `--remote`.
        host: RemoteAddr,
        /// The daemon's Z85 public key (40 characters). Z85 includes `-`, so a key may start
        /// with one; it is still read as the value.
        #[arg(long, allow_hyphen_values = true)]
        server_key: String,
        /// Name to authorize this client under (default: this host's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Show this client's key and the pinned daemon keys.
    Show,
}
