//! `ac2 osc …`, `ac2 cue …` and `ac2 rig …`: the rig monitor's cues and settings.

use std::net::SocketAddr;
use std::str::FromStr;

use ac2_proto::model::{DmxSet, OscArg, OscMessage, OscPath, PeakQuantity, parse_dmx_list};
use clap::{Args, Subcommand, ValueEnum};

use super::MeasRef;
use ac2_scene::unit_text::{BandNominalArg, LeqWindowArg, UnitError};

/// `osc …`.
#[derive(Debug, Subcommand)]
pub enum OscCmd {
    /// Where the rig sends its OSC cues: `192.168.1.50:9000` (an IP address, no host
    /// names), or `off` to send nothing. Kept by the rig, whatever session is open.
    Target {
        /// `ip:port` or `off`.
        addr: OscTargetArg,
    },
    /// Send one OSC message to the target now, to check the receiver hears it.
    #[command(after_help = OSC_ARGS_HELP)]
    Test {
        /// OSC address, e.g. `/light/red`.
        path: OscPathArg,
        /// Arguments, in order (see below).
        #[arg(allow_negative_numbers = true)]
        args: Vec<OscArgText>,
    },
}

/// `cue`.
#[derive(Debug, Args)]
pub struct CueArgs {
    /// SPL meter: id or name.
    pub meas: MeasRef,
    /// What the cue is about: `meter`, `window:<N|window>`, `peak:lcpeak|lafmax`,
    /// `band:<hz>:<window>`, `predicted`.
    pub subject: CueSubjectArg,
    /// OSC message in the ok state: `"/path arg …"`, or `off` for none.
    #[arg(long, value_name = "MSG", allow_hyphen_values = true)]
    pub ok: Option<CueMsgArg>,
    /// OSC message in the near state.
    #[arg(long, value_name = "MSG", allow_hyphen_values = true)]
    pub near: Option<CueMsgArg>,
    /// OSC message in the over state.
    #[arg(long, value_name = "MSG", allow_hyphen_values = true)]
    pub over: Option<CueMsgArg>,
    /// OSC message in the fault state.
    #[arg(long, value_name = "MSG", allow_hyphen_values = true)]
    pub fault: Option<CueMsgArg>,
    /// Art-Net levels held in the ok state: `"12=0 13=255"` (channel 1–512 = level 0–255),
    /// or `off` for none.
    #[arg(long, value_name = "LEVELS")]
    pub ok_dmx: Option<DmxListArg>,
    /// Art-Net levels held in the near state.
    #[arg(long, value_name = "LEVELS")]
    pub near_dmx: Option<DmxListArg>,
    /// Art-Net levels held in the over state.
    #[arg(long, value_name = "LEVELS")]
    pub over_dmx: Option<DmxListArg>,
    /// Art-Net levels held in the fault state.
    #[arg(long, value_name = "LEVELS")]
    pub fault_dmx: Option<DmxListArg>,
    /// The messages given here also carry the level the state was judged on, dB SPL, as a
    /// trailing float.
    #[arg(long)]
    pub append_level: bool,
    /// Remove the cue: nothing is sent for this subject.
    #[arg(long, conflicts_with_all = [
        "ok", "near", "over", "fault", "ok_dmx", "near_dmx", "over_dmx", "fault_dmx",
        "append_level",
    ])]
    pub clear: bool,
}

impl CueArgs {
    /// The OSC message and the levels given for `state` (`None`: not given, kept).
    pub fn given(
        &self,
        state: ac2_proto::model::CueState,
    ) -> (Option<&CueMsgArg>, Option<&DmxListArg>) {
        use ac2_proto::model::CueState as S;
        match state {
            S::Ok => (self.ok.as_ref(), self.ok_dmx.as_ref()),
            S::Near => (self.near.as_ref(), self.near_dmx.as_ref()),
            S::Over => (self.over.as_ref(), self.over_dmx.as_ref()),
            S::Fault => (self.fault.as_ref(), self.fault_dmx.as_ref()),
        }
    }
}

/// Art-Net levels: `channel=value …` (space- or comma-separated), or `off` for none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmxListArg(pub Vec<DmxSet>);

impl FromStr for DmxListArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        if s.trim().eq_ignore_ascii_case("off") {
            return Ok(Self(Vec::new()));
        }
        let l = parse_dmx_list(s).map_err(|e| UnitError(e.to_string()))?;
        if l.is_empty() {
            return Err(UnitError(
                "no levels: give channel=value (e.g. \"12=255 13=0\") or off".into(),
            ));
        }
        Ok(Self(l))
    }
}

const OSC_ARGS_HELP: &str = "\
Arguments: i:<int> (32-bit integer), f:<float> (32-bit float), s:<text> (string); a bare
number is an integer without a decimal point or exponent (`1`, `-3`) and a float with one
(`0.5`, `1e3`); any other bare word is a string (`red`).

Example: ac2 osc test /light/red i:1 0.75 s:stage";

pub(super) const CUE_HELP: &str = "\
Subjects: meter (the worst state of all its limits), window:<N> (the meter's Nth Leq window,
from 1) or window:<window> (`window:15min`, `window:c:30s`; A unless a weighting is given),
peak:lcpeak or peak:lafmax, band:<hz>:<window> (a band window: `band:63hz:60min`, Z unless
a weighting is given, `band:63hz:a:15min`), predicted (the band meter's predicted LAeq).

MSG: an OSC address and its arguments as one quoted argument: \"/light/red i:1\",
or `off` for no message in that state. Arguments: i:<int>, f:<float>, s:<text> (no spaces),
a bare number (integer without a decimal point, else float) or a bare word (string).

LEVELS: Art-Net channel=value pairs held while the state is current, as one quoted argument:
\"12=255 13=0\" (channel 1-512, value 0-255), or `off` for none. Each channel of the universe
is the highest level any cue's current state gives it, 0 where none does (`ac2 artnet`).

Example: ac2 cue Stage window:15min --near \"/light/amber i:1\" --over \"/light/red i:1\" \\
           --ok \"/light/off\" --fault \"/light/blink i:1\" --over-dmx \"12=255 13=0\" \\
           --ok-dmx \"12=0 13=255\"";

/// `rig …`.
#[derive(Debug, Subcommand)]
pub enum RigCmd {
    /// OSC and Art-Net targets and counters, the universe's lit channels, the monitor at
    /// start, and every rig SPL meter with the state of each of its cues.
    Status,
    /// With it on, a restarted daemon reopens the last session's device capture-only and
    /// the rig SPL meters meter with no client connected.
    MonitorAtStart {
        /// `on` or `off`.
        #[arg(value_enum)]
        on: OnOffArg,
    },
}

/// `on` / `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OnOffArg {
    /// On.
    On,
    /// Off.
    Off,
}

impl OnOffArg {
    /// Whether on.
    pub fn on(self) -> bool {
        self == Self::On
    }
}

/// A measurement's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ScopeArg {
    /// The rig's (SPL meters only): kept by the rig across session loads and restarts.
    Rig,
    /// The session's: saved in session files, replaced by a load.
    Session,
}

impl From<ScopeArg> for ac2_proto::model::MeasScope {
    fn from(s: ScopeArg) -> Self {
        match s {
            ScopeArg::Rig => Self::Rig,
            ScopeArg::Session => Self::Session,
        }
    }
}

/// `ip:port`, or `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OscTargetArg(pub Option<SocketAddr>);

impl FromStr for OscTargetArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        if s.trim().eq_ignore_ascii_case("off") {
            return Ok(Self(None));
        }
        s.trim()
            .parse::<SocketAddr>()
            .map(|a| Self(Some(a)))
            .map_err(|_| {
                UnitError(format!(
                    "{s:?}: expected ip:port (e.g. 192.168.1.50:9000, [fe80::1]:9000) or off"
                ))
            })
    }
}

/// An OSC address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OscPathArg(pub OscPath);

impl FromStr for OscPathArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        OscPath::new(s)
            .map(Self)
            .map_err(|e| UnitError(format!("{s:?}: {e}")))
    }
}

/// One OSC argument: `i:1`, `f:0.5`, `s:text`, a bare number or a bare word.
#[derive(Debug, Clone, PartialEq)]
pub struct OscArgText(pub OscArg);

impl FromStr for OscArgText {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let int = |v: &str| {
            v.trim()
                .parse::<i32>()
                .map_err(|_| UnitError(format!("{s:?}: not a 32-bit integer")))
        };
        let float = |v: &str| match v.trim().parse::<f32>() {
            Ok(f) if f.is_finite() => Ok(f),
            _ => Err(UnitError(format!("{s:?}: not a finite 32-bit float"))),
        };
        let arg = match s.split_once(':') {
            Some(("i", v)) => OscArg::Int(int(v)?),
            Some(("f", v)) => OscArg::Float(float(v)?),
            Some(("s", v)) => OscArg::Str(v.to_owned()),
            _ if looks_numeric(s) => {
                if s.contains(['.', 'e', 'E']) {
                    OscArg::Float(float(s)?)
                } else {
                    OscArg::Int(int(s)?)
                }
            }
            _ => OscArg::Str(s.to_owned()),
        };
        Ok(Self(arg))
    }
}

/// A word that is meant as a number: a sign or digit first, then a digit somewhere.
fn looks_numeric(s: &str) -> bool {
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    body.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        && body.contains(|c: char| c.is_ascii_digit())
}

/// A cue's message for one state: `"/path arg …"`, or `off`.
#[derive(Debug, Clone, PartialEq)]
pub struct CueMsgArg(pub Option<(OscPath, Vec<OscArg>)>);

impl CueMsgArg {
    /// The message, with `append_level`.
    pub fn message(&self, append_level: bool) -> Option<OscMessage> {
        self.0.as_ref().map(|(path, args)| OscMessage {
            path: path.clone(),
            args: args.clone(),
            append_level,
        })
    }
}

impl FromStr for CueMsgArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let mut words = s.split_whitespace();
        let Some(path) = words.next() else {
            return Err(UnitError(
                "an empty message: give \"/path arg …\" or off".into(),
            ));
        };
        if path.eq_ignore_ascii_case("off") && words.clone().next().is_none() {
            return Ok(Self(None));
        }
        let path = path.parse::<OscPathArg>()?.0;
        let args = words
            .map(|w| w.parse::<OscArgText>().map(|a| a.0))
            .collect::<Result<_, _>>()?;
        Ok(Self(Some((path, args))))
    }
}

/// What a cue is about, as given; resolved against the meter's config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueSubjectArg {
    /// The whole meter.
    Meter,
    /// A Leq window by position, from 1.
    WindowIndex(usize),
    /// A Leq window by length and weighting.
    Window(LeqWindowArg),
    /// A peak limit.
    Peak(PeakQuantity),
    /// A band window: the band (index into the 1/3-octave bands), its length and weighting
    /// (Z unless given).
    Band {
        /// Band index.
        band: usize,
        /// Length and weighting.
        window: LeqWindowArg,
    },
    /// The band meter's predicted window.
    Predicted,
}

impl FromStr for CueSubjectArg {
    type Err = UnitError;
    fn from_str(s: &str) -> Result<Self, UnitError> {
        let t = s.trim().to_ascii_lowercase();
        let expected = "expected meter, window:<N|window>, peak:lcpeak|lafmax, \
                        band:<hz>:<window> or predicted";
        match t.split_once(':') {
            None if t == "meter" => Ok(Self::Meter),
            None if t == "predicted" => Ok(Self::Predicted),
            Some(("window", w)) => match w.parse::<usize>() {
                Ok(0) => Err(UnitError(format!("{s:?}: windows count from 1"))),
                Ok(n) => Ok(Self::WindowIndex(n)),
                Err(_) => Ok(Self::Window(w.parse()?)),
            },
            Some(("peak", "lcpeak")) => Ok(Self::Peak(PeakQuantity::LcPeak)),
            Some(("peak", "lafmax")) => Ok(Self::Peak(PeakQuantity::LafMax)),
            Some(("peak", q)) => Err(UnitError(format!(
                "{s:?}: peak {q:?} (expected lcpeak or lafmax)"
            ))),
            Some(("band", rest)) => {
                let Some((hz, w)) = rest.split_once(':') else {
                    return Err(UnitError(format!(
                        "{s:?}: a band cue names the band and the window: band:63hz:60min"
                    )));
                };
                let band = hz.parse::<BandNominalArg>()?.0;
                let mut window: LeqWindowArg = w.parse()?;
                window.weighting = Some(window.weighting.unwrap_or(ac2_proto::model::Weighting::Z));
                Ok(Self::Band { band, window })
            }
            _ => Err(UnitError(format!("{s:?}: {expected}"))),
        }
    }
}
