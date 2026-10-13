//! `ac2 session …`: open, close, replay and mic/curve assignment.

use std::path::PathBuf;
use std::str::FromStr;

use clap::{Args, Subcommand};

use super::BackendArg;
use crate::units::{Channel, Channels, Freq, SampleCount};

/// `session …`.
#[derive(Debug, Subcommand)]
pub enum SessionCmd {
    /// Open the session on one device of an explicitly named backend.
    Open(SessionOpen),
    /// Close the session.
    Close,
    /// Current session.
    Status,
    /// Start a new session: no measurements or traces. Refused while there are unsaved
    /// changes, unless --discard (what it replaces stays recoverable: `session recover`).
    New {
        /// Replace unsaved changes.
        #[arg(long)]
        discard: bool,
    },
    /// Save measurements and traces (with slots and display edits) to the session they
    /// belong to (`session status`).
    Save,
    /// Save measurements and traces as another session; they then belong to it.
    SaveAs {
        /// Session name (in the daemon's session directory), a directory path on the
        /// daemon's host, or an .ac2session file on this machine.
        session: String,
        /// Replace an existing session of that name.
        #[arg(long)]
        force: bool,
    },
    /// Open a saved session: replaces measurements and traces; always comes up disarmed.
    /// Refused while there are unsaved changes, unless --discard.
    Load {
        /// Session name, a directory path on the daemon's host, or an .ac2session file (or
        /// session folder, for a daemon on another host) on this machine.
        session: String,
        /// Replace unsaved changes (they stay recoverable: `session recover`).
        #[arg(long)]
        discard: bool,
    },
    /// Bring back the measurements and traces the last `new` or `load` replaced while
    /// they had unsaved changes.
    Recover,
    /// Bring a session that cannot be opened (another format version) into the current
    /// measurements and traces. From a folder or .ac2session file on this machine: its
    /// measurements with their settings, colours, layouts and delays, its traces under
    /// their measurements, and the running ones started where the audio session captures
    /// their inputs, with a summary of what was restored, skipped or not read as saved.
    /// From a name or a path of the daemon's host: its traces only.
    Salvage {
        /// A session folder or .ac2session file here, or as for `load`.
        session: String,
    },
    /// Write the measurements and traces (or a saved session) to an .ac2session file on
    /// this machine.
    Export {
        /// File, or a folder for the suggested file name.
        file: PathBuf,
        /// A saved session instead of the current measurements and traces.
        #[arg(long)]
        session: Option<String>,
    },
    /// Store an .ac2session file or session folder from this machine on the daemon as a
    /// saved session, without opening it.
    Import {
        /// File or folder.
        file: PathBuf,
        /// Session name; the file's name without its extension when omitted.
        #[arg(long)]
        name: Option<String>,
        /// Replace an existing session of that name.
        #[arg(long)]
        force: bool,
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
    /// Open the device's driver settings on the daemon's host: an ASIO driver's own control
    /// panel (rate, buffer size, clock source, routing), or Audio MIDI Setup for a Core
    /// Audio device (rate, format, clock source). A rate change there resets the driver:
    /// the open session reopens at the new settings by itself (`session status` says what
    /// changed).
    Panel {
        /// Backend (default: the open session's).
        #[arg(long, value_enum)]
        backend: Option<BackendArg>,
        /// Device id or name (default: the open session's device, else the backend's
        /// only or default device).
        #[arg(long)]
        device: Option<String>,
    },
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
    /// Capture device id or name (default: the backend's only device).
    #[arg(long)]
    pub device: Option<String>,
    /// Playback device id or name, when it is another device than the capture one (a USB
    /// mic, an output-only device). Default: the capture device when it has outputs,
    /// else the system's default output.
    #[arg(long, value_name = "ID|NAME")]
    pub out_device: Option<String>,
    /// Input channels to capture, e.g. `1-4`.
    #[arg(long = "in", value_name = "CHANNELS")]
    pub inputs: Channels,
    /// Output channels of the stream.
    #[arg(long = "outputs", value_name = "N", default_value_t = 2)]
    pub outputs: u16,
    /// Sample rate, e.g. `48khz` (default: the device's rate).
    #[arg(long)]
    pub rate: Option<Freq>,
    /// Buffer size, e.g. `256samples` (default: the JACK server's or ASIO driver's own;
    /// Core Audio 1024 frames at 48 kHz, about 21 ms, scaled with the rate).
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
