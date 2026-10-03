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
    /// Input setup: the mic on each input and its mic-curve switch (shown without options).
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

/// `IN=on|off`: a 1-based input's mic-curve switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurveSwitch {
    /// Input.
    pub input: Channel,
    /// Apply the mic curve.
    pub on: bool,
}

impl FromStr for CurveSwitch {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("{s:?}: expected IN=on or IN=off, e.g. 3=off");
        let (ch, v) = s.split_once('=').ok_or_else(bad)?;
        let input: Channel = ch.parse().map_err(|e: crate::units::UnitError| e.0)?;
        let on = match v.trim().to_lowercase().as_str() {
            "on" => true,
            "off" => false,
            _ => return Err(bad()),
        };
        Ok(Self { input, on })
    }
}

/// `session inputs`.
#[derive(Debug, Args)]
pub struct SessionInputs {
    /// Mic on an input, e.g. `3=M30` (repeatable; `3=` clears the name).
    #[arg(long = "mic", value_name = "IN=NAME")]
    pub mics: Vec<MicAssign>,
    /// Mic-curve switch of an input, e.g. `3=off` (repeatable).
    #[arg(long = "curve", value_name = "IN=on|off")]
    pub curves: Vec<CurveSwitch>,
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
    /// Live SPL readout (q/Esc/Ctrl-C quits). Uses `--meas`, or its own SPL meter on
    /// `--input`, created for the duration of the command (Leq, Lmax and Lmin integrate
    /// from the command's start).
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
    /// Quit after this long, e.g. `10s` (scripts).
    #[arg(long = "for", value_name = "TIME")]
    pub duration: Option<Time>,
}

/// `cal …`.
#[derive(Debug, Subcommand)]
pub enum CalCmd {
    /// Calibrate an input against an acoustic calibrator.
    Spl(CalSpl),
    /// Import a mic's magnitude curve (.frd / .txt / CSV) for an input, or clear it.
    MicCurve(CalMicCurve),
    /// List calibrations and the input setup.
    List,
    /// Delete a calibration: its sensitivity, its mic curve, or both (the default).
    Rm(CalRm),
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
    /// Delete only the sensitivity calibration (keep the mic curve).
    #[arg(long, conflicts_with = "curve")]
    pub sensitivity: bool,
    /// Delete only the mic curve (keep the sensitivity calibration).
    #[arg(long)]
    pub curve: bool,
}

/// `cal mic-curve`.
#[derive(Debug, Args)]
pub struct CalMicCurve {
    /// Curve file: frequency and dB per line (further columns ignored).
    #[arg(required_unless_present = "clear", conflicts_with = "clear")]
    pub file: Option<PathBuf>,
    /// Input channel the mic is on.
    #[arg(long)]
    pub input: Channel,
    /// Mic name (default: the input's mic name in the session's input setup).
    #[arg(long)]
    pub mic: Option<String>,
    /// Remove the curve instead.
    #[arg(long)]
    pub clear: bool,
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
    /// A − B into a new trace: dB difference, or with `--complex` complex division A / B.
    Math {
        /// A.
        a: MeasRef,
        /// B.
        b: MeasRef,
        /// Name of the result.
        #[arg(long)]
        name: String,
        /// Complex division (keeps phase) instead of magnitude difference.
        #[arg(long)]
        complex: bool,
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
