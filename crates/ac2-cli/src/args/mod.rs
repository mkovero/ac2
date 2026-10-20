//! Command-line grammar.

mod artnet;
mod cal;
mod delay;
mod gen_;
mod ir;
mod math;
mod meas;
mod osc;
mod session;
mod spl;
mod trace;

pub use artnet::*;
pub use cal::*;
pub use delay::*;
pub use gen_::*;
pub use ir::*;
pub use math::*;
pub use meas::*;
pub use osc::*;
pub use session::*;
pub use spl::*;
pub use trace::*;

use std::path::PathBuf;

use ac2_client::RemoteAddr;
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::units::{Channel, Channels, Freq, LevelDbfs, SampleCount, Time};

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
    /// Audio I/O (open, close, status, inputs, replay) and saved session files.
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
    /// Where the rig sends OSC cues, and a test message.
    Osc {
        /// Action.
        #[command(subcommand)]
        cmd: OscCmd,
    },
    /// The rig's Art-Net universe: where it goes, and a test frame.
    Artnet {
        /// Action.
        #[command(subcommand)]
        cmd: ArtnetCmd,
    },
    /// What an SPL meter's limit (or the whole meter) does for external gear when its state
    /// changes: ok (under the limit), near (within the warn margin), over, fault (the rig
    /// cannot judge: no audio, device lost, not calibrated). Each state sends an OSC message
    /// and/or holds Art-Net levels. States not given keep theirs; with none given (and no
    /// --clear) it shows the cue.
    #[command(after_help = osc::CUE_HELP)]
    Cue(CueArgs),
    /// The rig monitor: its status, and whether a restarted daemon reopens the audio for the
    /// rig SPL meters.
    Rig {
        /// Action.
        #[command(subcommand)]
        cmd: RigCmd,
    },
    /// Daemon state.
    State {
        /// Action.
        #[command(subcommand)]
        cmd: StateCmd,
    },
    /// Hardware checks that run in this process on an audio backend, without a daemon.
    Selftest {
        /// Check.
        #[command(subcommand)]
        cmd: SelftestCmd,
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

/// `selftest …`.
#[derive(Debug, Subcommand)]
pub enum SelftestCmd {
    /// Duplex check of an audio interface: opens it, runs it for `--duration`, and prints a
    /// pass/fail report (device open, channel counts, block continuity, xruns, callback
    /// timing, both clocks against the host clock). Silent unless `--emit` is given; with
    /// `--emit` and a loopback cable it also measures output→input timing and drift. Stop
    /// the daemon first if it holds the device.
    Duplex(SelftestDuplex),
}

/// `selftest duplex`.
#[derive(Debug, Args)]
pub struct SelftestDuplex {
    /// Backend (no default: `fake` only when asked for).
    #[arg(long, value_enum)]
    pub backend: BackendArg,
    /// Capture device id or name (default: the system's default input).
    #[arg(long)]
    pub device: Option<String>,
    /// Playback device id or name, when it is another device than the capture one.
    /// Default: the capture device when it has outputs, else the system's default output.
    #[arg(long, value_name = "ID|NAME")]
    pub out_device: Option<String>,
    /// Input channels to capture, e.g. `1-8` (default: every input of the device).
    #[arg(long = "in", value_name = "CHANNELS")]
    pub inputs: Option<Channels>,
    /// Output channels to open (default: up to 2). They play silence unless `--emit`
    /// routes the stimulus to the loopback output.
    #[arg(long = "outputs", value_name = "N")]
    pub outputs: Option<u16>,
    /// Sample rate, e.g. `48khz` (default: device default).
    #[arg(long)]
    pub rate: Option<Freq>,
    /// Buffer size, e.g. `256samples` (default: the backend's choice).
    #[arg(long)]
    pub buffer: Option<SampleCount>,
    /// How long to run, e.g. `60s`.
    #[arg(long, default_value = "30s")]
    pub duration: Time,
    /// Play pink noise at this level (at most -20dbfs) on `--loopback-out` and time its
    /// return on `--loopback-in` through a cable. Without it the outputs stay silent.
    #[arg(
        long,
        value_name = "DBFS",
        allow_hyphen_values = true,
        requires_all = ["loopback_out", "loopback_in"]
    )]
    pub emit: Option<LevelDbfs>,
    /// Output channel the loopback cable starts at (with `--emit`).
    #[arg(long, requires = "emit")]
    pub loopback_out: Option<Channel>,
    /// Input channel the loopback cable returns on (with `--emit`).
    #[arg(long, requires = "emit")]
    pub loopback_in: Option<Channel>,
}

/// Audio backend, always named explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendArg {
    /// JACK (JACK2, or PipeWire through pipewire-jack): the real backend on Linux.
    Jack,
    /// The Core Audio HAL directly: the real backend on macOS (one callback per device,
    /// exact index).
    #[value(name = "coreaudio")]
    CoreAudio,
    /// ASIO drivers: the real backend on Windows.
    Asio,
    /// Simulated device; never touches hardware.
    Fake,
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
