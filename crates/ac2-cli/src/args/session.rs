//! `ac2 session …`: open, close, replay and the input setup (kinds, mics, curves, pairs).

use std::path::PathBuf;
use std::str::FromStr;

use clap::{Args, Subcommand};

use super::BackendArg;
use ac2_proto::model::{InputKind, Pair};
use ac2_scene::unit_text::{Channel, Channels, Freq, SampleCount};

/// `session …`.
#[derive(Debug, Subcommand)]
pub enum SessionCmd {
    /// Open the audio I/O on one device of an explicitly named backend.
    Open(SessionOpen),
    /// Close the audio I/O.
    Close,
    /// The audio I/O and the working session: name, unsaved changes, what could be recovered.
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
    /// their measurements, and the running ones started where the audio I/O captures
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
    /// Input setup: what each input carries (mic, line or digital), the mic's name and
    /// active curve, and the stereo pairs (shown without options).
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
        let input: Channel = ch
            .parse()
            .map_err(|e: ac2_scene::unit_text::UnitError| e.0)?;
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
        let input: Channel = ch
            .parse()
            .map_err(|e: ac2_scene::unit_text::UnitError| e.0)?;
        Ok(Self {
            input,
            curve: v.parse()?,
        })
    }
}

/// What an input carries, as the operator declares it: `mic`, `line` or `digital`. The
/// words are the ones the app's rows use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindArg {
    /// A measurement microphone (dB SPL through its calibration).
    Mic,
    /// Analogue line level (dBFS, dBu once line-calibrated).
    Line,
    /// A digital feed (dBFS, never calibrated).
    Digital,
}

impl KindArg {
    /// The protocol's kind: a mic starts unnamed with no curve chosen (the mic name and
    /// curve are set separately, and a line or digital input carries neither).
    pub fn to_kind(self) -> InputKind {
        match self {
            Self::Mic => InputKind::Mic {
                mic: None,
                curve: ac2_proto::model::CurveChoice::NotChosen,
            },
            Self::Line => InputKind::Line,
            Self::Digital => InputKind::Digital,
        }
    }
}

impl FromStr for KindArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mic" => Ok(Self::Mic),
            "line" => Ok(Self::Line),
            "digital" => Ok(Self::Digital),
            _ => Err(format!("{s:?}: expected mic, line or digital")),
        }
    }
}

/// `IN=mic|line|digital`: what a 1-based input carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindAssign {
    /// Input.
    pub input: Channel,
    /// Its kind.
    pub kind: KindArg,
}

impl FromStr for KindAssign {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (ch, k) = s
            .split_once('=')
            .ok_or_else(|| format!("{s:?}: expected IN=mic|line|digital, e.g. 3=line"))?;
        let input: Channel = ch
            .parse()
            .map_err(|e: ac2_scene::unit_text::UnitError| e.0)?;
        Ok(Self {
            input,
            kind: k.parse()?,
        })
    }
}

/// `L,R[=NAME]`: a stereo pair of two 1-based inputs, left first, optionally named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairArg {
    /// Left input.
    pub left: Channel,
    /// Right input.
    pub right: Channel,
    /// Operator's name; `None` = named by its inputs.
    pub name: Option<String>,
}

impl PairArg {
    /// The protocol's pair.
    pub fn to_pair(&self) -> Pair {
        Pair {
            left: self.left.0,
            right: self.right.0,
            name: self.name.clone(),
        }
    }
}

impl FromStr for PairArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("{s:?}: expected L,R or L,R=NAME, e.g. 3,4=Main");
        let (chs, name) = match s.split_once('=') {
            Some((c, n)) => (c, Some(n.trim())),
            None => (s, None),
        };
        let (l, r) = chs.split_once(',').ok_or_else(bad)?;
        let parse = |t: &str| -> Result<Channel, String> {
            t.parse().map_err(|e: ac2_scene::unit_text::UnitError| e.0)
        };
        Ok(Self {
            left: parse(l)?,
            right: parse(r)?,
            name: name.filter(|n| !n.is_empty()).map(str::to_owned),
        })
    }
}

/// `session inputs`. Kinds apply before mic names and curves, so `--kind 3=mic --mic 3=M30`
/// is one command; the pair list given replaces the daemon's.
#[derive(Debug, Default, Args)]
pub struct SessionInputs {
    /// What an input carries, e.g. `3=line`, `1=mic`, `5=digital` (repeatable). A new
    /// mic is unnamed; a line or digital input drops its mic name and curve.
    #[arg(long = "kind", value_name = "IN=mic|line|digital")]
    pub kinds: Vec<KindAssign>,
    /// Mic on an input, e.g. `3=M30` (repeatable; `3=` clears the name). Mic inputs only.
    #[arg(long = "mic", value_name = "IN=NAME")]
    pub mics: Vec<MicAssign>,
    /// Active mic curve of an input, e.g. `3=90°` or `3=off` (repeatable). Mic inputs only.
    #[arg(long = "curve", value_name = "IN=LABEL|off")]
    pub curves: Vec<CurveAssign>,
    /// A stereo pair, left input first, e.g. `3,4` or `3,4=Main` (repeatable): the pairs
    /// given replace the current ones. Two inputs of one kind: mics, line or digital.
    #[arg(long = "pair", value_name = "L,R[=NAME]", conflicts_with = "no_pairs")]
    pub pairs: Vec<PairArg>,
    /// Remove every pair.
    #[arg(long)]
    pub no_pairs: bool,
}

impl SessionInputs {
    /// Nothing to change: the command only lists the setup.
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
            && self.mics.is_empty()
            && self.curves.is_empty()
            && self.pairs.is_empty()
            && !self.no_pairs
    }
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
    /// Spare inputs to open (JACK): ports tied to no physical input that you route into
    /// with your own JACK tools, numbered after the physical inputs and always captured.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub spare_inputs: u16,
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
