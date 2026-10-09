//! Application state and its reducer.
//!
//! [`AppState::update`] is pure: a message in, state changed, [`Request`]s for the daemon
//! link out. No clock, no socket, no egui — every keyboard flow (including the stimulus
//! arm → fire → stop cluster) is tested here without a window.
//!
//! The UI computes no measurement values: it keeps operator choices (view ranges, display
//! offsets, polarity, nudges, selection) and hands them with the received frames to
//! `ac2-scene`, which does all display math.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ac2_client::MirrorView;
use ac2_proto::Command;
use ac2_proto::GridDef;
use ac2_proto::model::{
    AverageMethod, CalKey, CurveChoice, DelayFinding, DelayOutcome, DelayPick, DelayReference,
    FinderBand, GeneratorDesired, GeneratorSettings, ImportRole, InputSetup, MathDomain, MeasKind,
    Measurement, MicCurveId, Operand, Polarity, SessionRef, Signal, Smoothing, SmoothingFraction,
    SmoothingMode, State, SweepStatus, TraceData, TraceKind, TraceMeta, TraceOwner,
};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{ClientId, Db, Dbfs, Hz, MeasId, Seconds, SweepId, TraceId};
pub use ac2_scene::banner::Severity;
use ac2_scene::spectrum::PeakHold;
use ac2_scene::stimulus::{Drive, Next as NextKey, Stimulus as NextStimulus};
use ac2_scene::theme::ThemeName;
use ac2_scene::trace::TraceKey;
use ac2_scene::view::{
    CoherencePlacement, DistortionUnit, FreqRange, IrMode, LeqStyle, PhaseView, PlotChrome,
    SpectrumMode, SpectrumStyle, SplMode, SweepMode, ViewState,
};
use ac2_scene::{axis::Range, format};

use crate::anim::FreqNav;
use crate::cal_view::{CalAction, CalView};
use crate::conn::{ConnEvent, DataSnapshot, Request, StimEvent};
use crate::forms::{Form, FormKind};
use crate::keys::{Chord, CommandId, Keymap, RESERVED, STOP_ANYWHERE, Scope};
use crate::leq_dialog::LeqDialog;
use crate::palette::Palette;
use crate::prefs::UiPrefs;
use crate::session_dialog::{RoleKey, Row, SessionDialog};
use crate::settings::{Page, Settings};

#[path = "state_display.rs"]
mod display;
pub use display::{
    ChoicePrompt, ChoicePurpose, DeletePrompt, DeleteTarget, LEVEL_ZOOM_FACTOR, MoveWhat,
    level_range,
};
#[path = "state_settings.rs"]
mod settings_impl;
pub use settings_impl::SettingsMsg;
#[path = "state_follow.rs"]
mod follow;
pub use follow::panes_drawing;
#[path = "state_ir.rs"]
mod ir_nav;
pub use ir_nav::IrNavMsg;
#[path = "state_band_transfer.rs"]
mod band_transfer;
#[path = "state_commands.rs"]
mod commands;
#[path = "state_dialogs.rs"]
mod dialogs;
#[path = "state_keys.rs"]
mod keys;
#[path = "state_leq.rs"]
mod leq;
#[path = "state_link.rs"]
mod link;
#[path = "state_panes.rs"]
mod panes;
#[path = "state_queries.rs"]
mod queries;
#[path = "state_stimulus.rs"]
mod stimulus;
#[path = "state_text.rs"]
mod text;
pub use panes::{
    Axis, DEFAULT_PANE_AREA, Layout, PaneId, PaneMenuRow, PaneModes, PaneNode, PaneRect, View,
};
use text::{SELECT_TRACE_FIRST, drawn_in, offset_text, slot_of};
pub use text::{
    curve_what, meas_input, mics_text, open_session_hint, parse_band, parse_mics, parse_number,
    parse_outputs, parse_session_ref, parse_slot, trace_label, transfer_kind_shown,
};

/// The panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneKind {
    Transfer,
    Spectrum,
    Ir,
    Spl,
    /// Sweep results: response, harmonic distortion, the sweep's IR. Hidden until a sweep
    /// is stored (or the operator shows it).
    Distortion,
}

impl PaneKind {
    pub const ALL: [PaneKind; 5] = [
        PaneKind::Transfer,
        PaneKind::Spectrum,
        PaneKind::Ir,
        PaneKind::Spl,
        PaneKind::Distortion,
    ];

    pub fn scope(self) -> Scope {
        match self {
            PaneKind::Transfer => Scope::Transfer,
            PaneKind::Spectrum => Scope::Spectrum,
            PaneKind::Ir => Scope::Ir,
            PaneKind::Spl => Scope::Spl,
            PaneKind::Distortion => Scope::Distortion,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            PaneKind::Transfer => "Transfer",
            PaneKind::Spectrum => "Spectrum / RTA",
            PaneKind::Ir => "Impulse response",
            PaneKind::Spl => "SPL",
            PaneKind::Distortion => "Sweep / distortion",
        }
    }

    /// Whether the pane shows measurements of kind `k` (the IR pane shows the transfer
    /// pane's measurement).
    pub fn shows(self, k: &MeasKind) -> bool {
        match self {
            // A sweep measurement has no live curve, but its runs are transfer curves: the
            // pane draws them when the sweep is its measurement.
            PaneKind::Transfer => k.publishes_tf() || matches!(k, MeasKind::Sweep { .. }),
            // A math channel has no impulse response of its own.
            PaneKind::Ir => matches!(k, MeasKind::Transfer { .. }),
            PaneKind::Spectrum => k.publishes_levels(),
            PaneKind::Spl => matches!(k, MeasKind::Spl { .. }),
            // Sweep measurements: the pane shows their runs.
            PaneKind::Distortion => matches!(k, MeasKind::Sweep { .. }),
        }
    }

    /// The pane whose measurement choice this one follows.
    pub fn owner(self) -> PaneKind {
        match self {
            PaneKind::Ir => PaneKind::Transfer,
            p => p,
        }
    }

    /// The pane that shows measurements of kind `k`.
    pub fn for_kind(k: &MeasKind) -> PaneKind {
        match k.stream() {
            Some(Stream::Spec | Stream::Rta) => PaneKind::Spectrum,
            Some(Stream::Spl) => PaneKind::Spl,
            None => PaneKind::Distortion,
            _ => PaneKind::Transfer,
        }
    }

    /// What the pane's measurements are, for messages.
    pub fn what(self) -> &'static str {
        match self {
            PaneKind::Transfer | PaneKind::Ir => "transfer",
            PaneKind::Spectrum => "spectrum or RTA",
            PaneKind::Spl => "SPL",
            PaneKind::Distortion => "sweep",
        }
    }
}

/// Smoothing steps K / Shift+K walk through, finest first.
pub const SMOOTHING_STEPS: [Option<SmoothingFraction>; 6] = [
    None,
    Some(SmoothingFraction::FortyEighth),
    Some(SmoothingFraction::TwentyFourth),
    Some(SmoothingFraction::Twelfth),
    Some(SmoothingFraction::Sixth),
    Some(SmoothingFraction::Third),
];

/// A spectrum's smoothing as a trace edit: power only (a spectrum has no phase).
pub fn spectrum_smoothing(fraction: SmoothingFraction) -> Smoothing {
    Smoothing {
        fraction,
        mode: SmoothingMode::Magnitude,
    }
}

/// What the smoothing keys change: the selected slot's trace, or the focused pane's
/// measurement (the spectrum pane's when it has focus, else the transfer pane's).
#[derive(Clone, Debug, PartialEq)]
pub enum SmoothTarget {
    Trace(TraceMeta),
    Meas(Measurement),
}

/// What smoothing does to a curve of some kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Smoothable {
    /// A transfer function: magnitude, and phase unless the mode keeps it.
    Transfer,
    /// A narrowband spectrum: power over a fractional-octave kernel.
    Spectrum,
    /// RTA bands: already fractional-octave.
    Rta,
    /// Not smoothed (a target curve, a meter).
    No,
}

impl SmoothTarget {
    pub fn kind(&self) -> Smoothable {
        match self {
            SmoothTarget::Trace(t) => match t.kind {
                TraceKind::Transfer | TraceKind::Sweep => Smoothable::Transfer,
                TraceKind::Spectrum { .. } => Smoothable::Spectrum,
                TraceKind::Rta { .. } => Smoothable::Rta,
                TraceKind::Target => Smoothable::No,
            },
            SmoothTarget::Meas(m) => match &m.config.kind {
                MeasKind::Transfer { .. } => Smoothable::Transfer,
                MeasKind::Spectrum { .. } => Smoothable::Spectrum,
                MeasKind::Rta { .. } => Smoothable::Rta,
                MeasKind::Spl { .. } | MeasKind::Sweep { .. } => Smoothable::No,
                MeasKind::Math { config } => match config.domain {
                    MathDomain::Transfer => Smoothable::Transfer,
                    MathDomain::Spectrum => Smoothable::Spectrum,
                    MathDomain::Rta => Smoothable::Rta,
                },
            },
        }
    }

    /// The pane its curve is drawn in.
    pub fn pane(&self) -> PaneKind {
        match self.kind() {
            Smoothable::Spectrum | Smoothable::Rta => PaneKind::Spectrum,
            Smoothable::Transfer | Smoothable::No => PaneKind::Transfer,
        }
    }

    pub fn smoothing(&self) -> Option<Smoothing> {
        match self {
            SmoothTarget::Trace(t) => t.edit.smoothing,
            SmoothTarget::Meas(m) => match &m.config.kind {
                MeasKind::Transfer { config } => config.smoothing,
                MeasKind::Math { config } => config.smoothing,
                MeasKind::Spectrum { config } => config.smoothing.map(spectrum_smoothing),
                _ => None,
            },
        }
    }

    /// `smoothing 1/6 oct`, `smoothing off`; a transfer function kept at measured phase
    /// says `mag only`, a spectrum (no phase) never does.
    pub fn caption(&self) -> String {
        let s = self.smoothing();
        match (self.kind(), s) {
            (Smoothable::Spectrum, Some(s)) => {
                format!("smoothing {}", format::octave_fraction(s.fraction))
            }
            _ => ac2_scene::tf::smoothing_caption(s),
        }
    }

    /// `slot 3 (Main L S3)`, the trace's name, or the measurement name.
    pub fn label(&self) -> String {
        match self {
            SmoothTarget::Trace(t) => trace_label(t),
            SmoothTarget::Meas(m) => m.config.name.clone(),
        }
    }
}

/// The measurement list a pane's title chip opens: the pane's compatible measurements, one
/// highlighted (Up/Down move, Enter shows it, Esc closes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneMenu {
    pub pane: PaneId,
    pub index: usize,
}

/// Link status as shown in the top bar.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Connecting {
        target: String,
    },
    Connected {
        target: String,
        server: String,
        client_id: ClientId,
    },
    Failed {
        target: String,
        error: String,
    },
}

/// Display edits of a live trace: offset and polarity are display only and go to the
/// scene, which applies them. Its delay is the measurement's, in the daemon.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveEdit {
    pub offset_db: f64,
    pub inverted: bool,
}

/// What a measurement stop's or delete's toast adds when the stimulus stopped with it.
pub(crate) const STIMULUS_STOPPED_TOO: &str =
    " · stimulus stopped (no transfer measurement left running)";

/// Where the operator's stimulus stands, as far as this client knows. The generator's
/// truth is the mirrored `Generator`; this tracks requests in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StimPhase {
    Idle,
    /// Lease + arm requested.
    Arming,
    /// Lease held, armed, silent.
    Armed,
    /// Fire requested.
    FireRequested,
    Firing,
    /// Stop requested.
    Stopping,
}

/// Stimulus settings chosen by the operator.
#[derive(Clone, Debug, PartialEq)]
pub struct Stimulus {
    /// Typed by the operator; `None` until then — nothing arms without it.
    pub level: Option<Dbfs>,
    pub signal: Signal,
    /// Zero-based output channels.
    pub outputs: Vec<u16>,
    pub phase: StimPhase,
    /// Space came while a stop was still on its way: the stimulus arms once that stop has
    /// landed (not into the lease the stop is releasing). Any stop, a lost or failed lease,
    /// a closed window and a new connection drop it.
    pub arm_after_stop: bool,
    /// The stop in flight went out with a measurement stop whose reply already says the
    /// stimulus stopped: its own "stimulus stopped" would say it twice.
    pub stop_announced: bool,
}

impl Default for Stimulus {
    fn default() -> Self {
        Self {
            level: None,
            signal: Signal::Pink,
            outputs: vec![0],
            phase: StimPhase::Idle,
            arm_after_stop: false,
            stop_announced: false,
        }
    }
}

impl Stimulus {
    fn settings(&self) -> Option<GeneratorSettings> {
        Some(GeneratorSettings {
            signal: self.signal,
            level: self.level?,
            band: None,
            outputs: self.outputs.clone(),
        })
    }

    /// `pink noise −20.0 dBFS → out 1`.
    pub fn describe(&self) -> String {
        let sig = match self.signal {
            Signal::White => "white noise".to_string(),
            Signal::Pink => "pink noise".to_string(),
            Signal::PeriodicPink { .. } => "periodic pink".to_string(),
            Signal::Sine { freq } => format!("sine {}", format::freq_readout(freq.0)),
            Signal::Ess { sweep } => format!(
                "sweep {} – {} {} s",
                format::freq_readout(sweep.start.0),
                format::freq_readout(sweep.end.0),
                format::fixed(sweep.duration.0, 1)
            ),
        };
        let level = self
            .level
            .map_or_else(|| "no level".to_string(), |l| dbfs(l.0));
        format!("{sig} {level} → out {}", outputs_text(&self.outputs))
    }
}

/// What X / Shift+X ask the delay finder for: the band and the observation (block length).
/// Auto band and automatic observation by default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FinderChoice {
    pub band: FinderBand,
    /// `None`: the band's default (decision D2: sub 4 s).
    pub observation: Option<Seconds>,
}

impl Default for FinderChoice {
    fn default() -> Self {
        Self {
            band: FinderBand::Auto,
            observation: None,
        }
    }
}

/// Sub-band observations the finder accepts (decision D2).
pub const SUB_OBSERVATIONS_S: [f64; 3] = [2.0, 4.0, 8.0];
/// Longest observation the finder accepts.
pub const MAX_OBSERVATION_S: f64 = 8.0;

impl FinderChoice {
    /// The band analysed needs a sub-band observation (2, 4 or 8 s): the sub preset, or a
    /// custom band starting below 150 Hz.
    fn sub_like(&self) -> bool {
        match self.band {
            FinderBand::Sub => true,
            FinderBand::Custom { lo_hz, .. } => lo_hz.0 < 150.0,
            _ => false,
        }
    }

    /// `auto band · auto observation`, `sub band · 8 s`, `80 Hz – 800 Hz · auto observation`.
    pub fn describe(&self) -> String {
        let band = match self.band {
            FinderBand::Auto => "auto band".to_string(),
            FinderBand::Full => "full band".to_string(),
            FinderBand::Mid => "mid band".to_string(),
            FinderBand::Sub => "sub band".to_string(),
            FinderBand::Custom { lo_hz, hi_hz } => format!(
                "{} – {}",
                format::freq_readout(lo_hz.0),
                format::freq_readout(hi_hz.0)
            ),
        };
        let obs = self.observation.map_or_else(
            || "auto observation".to_string(),
            |o| format!("{} s", format::fixed(o.0, 1).trim_end_matches(".0")),
        );
        format!("{band} · {obs}")
    }
}

fn dbfs(v: f64) -> String {
    format!("{} dBFS", format::signed(v, 1))
}

/// One-based channel list: `1, 2`.
pub fn outputs_text(o: &[u16]) -> String {
    o.iter()
        .map(|c| (u32::from(*c) + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The curve `text` names in the mic library: `MM1 34804 90°` (a mic name, then one of its
/// labels), or just the mic name when it has one curve.
pub fn parse_curve(mics: &[ac2_proto::model::Mic], text: &str) -> Result<MicCurveId, String> {
    let t = text.trim();
    // The longest mic name the text starts with: names may contain spaces, and one name may
    // begin another.
    let m = mics
        .iter()
        .filter(|m| t.starts_with(m.name.as_str()))
        .max_by_key(|m| m.name.len())
        .ok_or_else(|| {
            let names: Vec<&str> = mics.iter().map(|m| m.name.as_str()).collect();
            if names.is_empty() {
                "the mic library is empty: import a curve first (palette: Calibrations…)".into()
            } else {
                format!(
                    "no mic of the library starts {t:?} (mics: {})",
                    names.join(", ")
                )
            }
        })?;
    let rest = t[m.name.len()..].trim();
    let label = match (rest, m.curves.as_slice()) {
        ("", [only]) => only.label.clone(),
        ("", _) => {
            return Err(format!(
                "{} has several curves: add the label ({})",
                m.name,
                ac2_scene::cal::labels(&m.curves)
            ));
        }
        (l, curves) if curves.iter().any(|c| c.label == l) => l.to_owned(),
        (l, _) => {
            return Err(format!(
                "{} has no curve {l:?} (stored: {})",
                m.name,
                ac2_scene::cal::labels(&m.curves)
            ));
        }
    };
    Ok(MicCurveId {
        mic: m.name.clone(),
        label,
    })
}

/// The `trace.mic_curve` request of the trace mic-curve prompt: a curve of the mic library
/// (`MM1 34804 90°`), or `none`.
fn trace_mic_request(
    mics: &[ac2_proto::model::Mic],
    id: TraceId,
    text: &str,
) -> Result<Request, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("type the mic and curve, e.g. MM1 34804 90° (none removes the curve)".into());
    }
    let curve = if t.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(parse_curve(mics, t)?)
    };
    Ok(Request::Call {
        what: match &curve {
            Some(c) => format!(
                "trace {id}: mic curve {} applied",
                ac2_scene::cal::curve_name(&c.mic, &c.label)
            ),
            None => format!("trace {id}: mic curve removed"),
        },
        cmd: Command::TraceMicCurve { trace: id, curve },
    })
}

/// `2=90°`, `2=off` (1-based input): an input and the curve it applies.
pub fn parse_input_curve(text: &str) -> Result<(u16, CurveChoice), String> {
    let (ch, label) = text.split_once('=').ok_or_else(|| {
        format!(
            "{:?}: expected input=curve, e.g. 2=90° or 2=off",
            text.trim()
        )
    })?;
    let n: u16 = ch
        .trim()
        .parse()
        .map_err(|_| format!("not an input number: {:?}", ch.trim()))?;
    if n == 0 {
        return Err("inputs count from 1".into());
    }
    let l = label.trim();
    let choice = if l.eq_ignore_ascii_case("off") || l.eq_ignore_ascii_case("none") {
        CurveChoice::Off
    } else if l.is_empty() {
        return Err("type the curve label after = (or off)".into());
    } else {
        CurveChoice::Curve {
            label: l.to_owned(),
        }
    };
    Ok((n - 1, choice))
}

/// Longest recording the record toggle starts, s: an hour of every input bounds the file
/// (≈ 0.7 GB per channel at 48 kHz) if nobody stops it.
pub const RECORD_MAX_S: f64 = 3600.0;

/// What a text prompt sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    StimulusLevel,
    Offset(MeasId),
    Delay(MeasId),
    /// A file to import as a trace or target curve.
    ImportFile(ImportRole),
    SessionSave,
    SessionLoad,
    /// A recording to replay as the session.
    ReplayRecording,
    InputMics,
    /// `input=curve` of the input setup.
    MicCurveInput,
    /// `input=mic` of a sensitivity calibration on the session's capture device to delete.
    CalDelete,
    /// Mic whose curve goes on a stored trace (`none` removes the applied one).
    TraceMicCurve(TraceId),
    /// Display offset of a stored trace.
    TraceOffset(TraceId),
    /// The slot a stored trace moves to (`none` frees it).
    TraceSlot(TraceId),
    /// A stored trace's new name.
    TraceRename(TraceId),
    /// Where a stored trace is exported to.
    TraceExport(TraceId),
    /// Custom delay-finder band edges.
    FinderBand,
    /// Delay-finder observation.
    FinderObservation,
}

impl PromptKind {
    pub fn label(self) -> &'static str {
        match self {
            PromptKind::StimulusLevel => "Stimulus level (dBFS)",
            PromptKind::Offset(_) => "Display offset (dB)",
            PromptKind::TraceOffset(_) => "Display offset of the selected trace (dB)",
            PromptKind::TraceSlot(_) => {
                "Slot for the selected trace: 1 … 9 (its holder gives it up), none frees it"
            }
            PromptKind::TraceRename(_) => "New name for the selected trace",
            PromptKind::TraceExport(_) => {
                "Export the selected trace as ac2 CSV to (file path; a folder takes the trace's name)"
            }
            PromptKind::Delay(_) => "Delay (ms)",
            PromptKind::ImportFile(ImportRole::Target) => "Target curve file (path)",
            PromptKind::ImportFile(ImportRole::Trace) => "Trace file to import (path)",
            PromptKind::SessionSave => "Save session as (name or path)",
            PromptKind::SessionLoad => "Load session, disarmed (name or path)",
            PromptKind::ReplayRecording => "Replay recording (name or path of its .wav)",
            PromptKind::InputMics => "Mic per input (1-based, e.g. 3=M30, 4=ECM; 3= clears)",
            PromptKind::MicCurveInput => {
                "Mic curve on input N: input=curve (1-based, e.g. 2=90°; 2=off)"
            }
            PromptKind::CalDelete => {
                "Delete sensitivity calibration of input=mic on this device (e.g. 3=M30)"
            }
            PromptKind::TraceMicCurve(_) => {
                "Mic curve for the selected trace: mic and curve, e.g. MM1 34804 90° (none removes)"
            }
            PromptKind::FinderBand => "Delay finder band edges (Hz, e.g. 80-800)",
            PromptKind::FinderObservation => {
                "Delay finder observation (s; empty = automatic; sub band: 2, 4 or 8)"
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    pub error: Option<String>,
}

/// An ambiguous delay finding waiting for the operator (decision 1c): keys 1–3 insert a
/// candidate (1 is the first-arrival rule's pick).
#[derive(Clone, Debug, PartialEq)]
pub struct DelayChoice {
    pub meas: MeasId,
    pub name: String,
    pub finding: DelayFinding,
    /// The candidate ↑/↓ highlight and Enter inserts.
    pub selected: usize,
}

impl DelayChoice {
    /// Candidates the keys pick from.
    pub fn rows(&self) -> Vec<ac2_scene::finding::PickRow> {
        ac2_scene::finding::pick_rows(&self.finding)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Overlay {
    #[default]
    None,
    Help,
    /// The recent notifications, newest first: a message that went by is never lost.
    Notifications,
    Palette(Palette),
    Prompt(Prompt),
    /// Candidate list of an ambiguous finding over the transfer pane. 1–3 or ↑/↓ and Enter
    /// insert a candidate; other keys keep working except the stimulus's; Esc closes it.
    DelayPick(Box<DelayChoice>),
    /// A new-measurement dialog.
    Form(Box<Form>),
    /// The Settings view: every setting, a page per area ([`crate::settings`]).
    Settings(Box<Settings>),
    /// A pane's measurement list, opened from its title chip.
    PaneMenu(PaneMenu),
    /// After a session opened with a reference and mics on a daemon without measurements:
    /// one key creates a transfer measurement per mic.
    Offer(Box<Offer>),
    /// The confirmation before a new SPL log.
    NewLog(Box<NewLogPrompt>),
    /// The confirmation before the selected measurement or stored trace is deleted (or,
    /// for a measurement a math channel computes from, why it cannot be).
    Delete(Box<DeletePrompt>),
    /// A question with a few answers (←/→ or ↑/↓ pick, Enter takes it, Esc cancels):
    /// what deleting a measurement does with its traces, where a trace moves.
    Choose(Box<ChoicePrompt>),
}

impl Overlay {
    /// A window to read that leaves the keys working (except the stimulus's and those
    /// that scroll it), with nothing typed into it.
    pub fn is_reading(&self) -> bool {
        matches!(self, Overlay::Help | Overlay::Notifications)
    }

    /// The Settings view, when open.
    pub fn settings(&self) -> Option<&Settings> {
        match self {
            Overlay::Settings(s) => Some(s),
            _ => None,
        }
    }

    /// The session model while Settings shows the Inputs & outputs or Audio page.
    pub fn session(&self) -> Option<&SessionDialog> {
        match self {
            Overlay::Settings(s) if matches!(s.page, Page::Io | Page::Audio) => Some(&s.session),
            _ => None,
        }
    }

    pub fn session_mut(&mut self) -> Option<&mut SessionDialog> {
        match self {
            Overlay::Settings(s) if matches!(s.page, Page::Io | Page::Audio) => {
                Some(&mut s.session)
            }
            _ => None,
        }
    }

    /// The calibrations while Settings shows the Calibration page.
    pub fn cal(&self) -> Option<&CalView> {
        match self {
            Overlay::Settings(s) if s.page == Page::Calibration => Some(&s.cal),
            _ => None,
        }
    }

    /// The Leq windows while Settings shows the SPL / Leq page.
    pub fn leq(&self) -> Option<&LeqDialog> {
        match self {
            Overlay::Settings(s) if s.page == Page::Leq => s.leq.as_ref(),
            _ => None,
        }
    }

    pub fn leq_mut(&mut self) -> Option<&mut LeqDialog> {
        match self {
            Overlay::Settings(s) if s.page == Page::Leq => s.leq.as_mut(),
            _ => None,
        }
    }
}

/// What an SPL meter's history was rebuilt from its log for.
#[derive(Clone, Debug, PartialEq)]
struct LeqLogSeen {
    /// The windows it was rebuilt for.
    config: ac2_proto::model::LeqConfig,
    /// Rows logged as of the newest `leq` frame: a new log numbers its rows from 0 again.
    logged: u64,
    /// The log held a second (the `spl_log` entity's `started_at`): `spl.log_new` empties it.
    started: bool,
    /// The rebuild asked for; an answer to an earlier one is stale.
    ask: u64,
}

/// The confirmation before `spl.log_new`: which meter, and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct NewLogPrompt {
    pub meas: MeasId,
    pub meter: String,
    pub confirm: ac2_scene::leq::NewLogConfirm,
}

/// Transfer measurements offered after the session dialog opened a session.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub transfers: Vec<ac2_proto::model::MeasConfig>,
}

/// Seconds between renewals of the device preview (the daemon closes one not renewed
/// within 5 s).
pub const PREVIEW_RENEW_S: f64 = 2.0;

/// A notification in the window's corner ([`ac2_scene::toast`] lays it out and says how
/// long it stays).
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    /// Names it for a click that dismisses it.
    pub id: u64,
    pub text: String,
    pub severity: Severity,
    pub until_s: f64,
}

/// A notification as the log keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    pub severity: Severity,
    /// When it last came ([`AppState::now_s`]).
    pub at_s: f64,
    /// How many times in a row it came.
    pub count: u32,
}

/// Whether the Warning toasts setting keeps a notification out of the corner (the log
/// keeps it either way).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Muted {
    Never,
    WithWarningToasts,
}

/// Most toasts kept up at once; older ones go first (the log keeps them).
pub const MAX_TOASTS: usize = 32;
/// Coherence mask thresholds `B` cycles through (decision: blanking below γ²).
pub const COHERENCE_MASKS: [Option<f32>; 5] = [None, Some(0.3), Some(0.5), Some(0.7), Some(0.9)];
/// Pan step, octaves.
pub const PAN_OCTAVES: f64 = 1.0 / 3.0;
/// Zoom step: half an octave of span per key press each side.
pub const ZOOM_FACTOR: f64 = 1.5;
/// Plain `,` / `.` step: a measurement's delay, or a stored trace's nudge.
pub const NUDGE_S: f64 = 0.000_1;

/// Fine step of a measurement's own delay, in samples: 0.1 sample is 7.5° at 10 kHz and
/// 48 kHz, fine enough to align by the phase trace by eye.
pub const DELAY_FINE_STEP: f64 = 0.1;
/// How far ↑/↓ scroll the help overlay, points (about a row).
pub const HELP_LINE: f32 = 20.0;
/// PageUp / PageDown in the help overlay until the view has measured its page.
pub const HELP_PAGE: f32 = 400.0;
/// Lines PageUp / PageDown move in the calibrations view.
pub const CAL_PAGE: i32 = 10;
/// Lowest stimulus level the arrows go to.
pub const LEVEL_FLOOR: f64 = -90.0;

/// The mouse on the transfer pane's legend; places and limits as the scene's
/// [`ac2_scene::legend::LegendBox`] computed them from the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LegendMsg {
    /// The pointer came onto the plate or its grip, or left it.
    Hover(Option<ac2_scene::legend::LegendHover>),
    /// Dragged: its place, [`ac2_scene::legend::LegendView::x`] and `y`.
    Move { x: f32, y: f32 },
    /// Its grip dragged: its size limits.
    Resize { max_width: f32, max_height: f32 },
    /// The wheel over it: the first row shown.
    Scroll { first: usize },
}

/// Messages into the reducer.
#[derive(Clone, Debug)]
pub enum Msg {
    /// A key press (after the egui layer turned it into a chord).
    Key(Chord),
    /// Typed text (only used by the palette and prompts).
    Text(String),
    Backspace,
    /// The mouse wheel over a window that lists rows (the palette, a pane's measurement
    /// list): rows down (negative up), as ↑/↓ move.
    Wheel {
        rows: i32,
    },
    Command(CommandId),
    Conn(Box<ConnEvent>),
    /// Frame tick: `now_s` monotonic seconds, `dt_s` since the previous tick.
    Tick {
        now_s: f64,
        dt_s: f64,
    },
    /// The pointer came onto (`true`) or left the toasts.
    ToastsHeld(bool),
    /// A toast clicked away.
    DismissToast(u64),
    /// A measurement clicked in the list: selected, and shown by its pane.
    SelectMeas(MeasId),
    /// A stored trace clicked in the list: selected, so the trace keys act on it (again:
    /// deselected).
    SelectTrace(TraceId),
    /// A stored trace's eye in the list: shown / hidden.
    ToggleShown(TraceId),
    /// A stored trace double-clicked in the list: selected, and its name asked for.
    RenameTrace(TraceId),
    /// A click in the spectrograph: the cursor at `hz`, `before_s` seconds before the
    /// newest frame.
    SpectrographCursor {
        hz: f64,
        before_s: f64,
    },
    /// A click in a pane: focuses it and selects the measurement it shows.
    FocusPane(PaneId),
    /// The pane title chip: opens (or closes) the pane's list.
    PaneMenu(PaneId),
    /// A row picked from a pane's list: the pane shows that measurement (selected), or
    /// turns into that kind of pane.
    PanePick(PaneId, PaneMenuRow),
    /// Mouse wheel / pinch on a frequency axis.
    Zoom {
        about_hz: f64,
        factor: f64,
    },
    /// Mouse drag on a frequency axis.
    Pan {
        octaves: f64,
    },
    CursorAt(Option<f64>),
    /// The dB | % toggle of the distortion pane.
    DistortionUnit(DistortionUnit),
    /// The mouse on the transfer pane's legend.
    Legend(LegendMsg),
    /// Mouse on an open dialog.
    Form(FormMsg),
    /// Mouse on the session dialog.
    Session(SessionMsg),
    /// Mouse on the measurement offer: `true` creates, `false` skips.
    Offer(bool),
    /// Mouse on the new SPL log confirmation: `true` starts it, `false` keeps the log.
    NewLog(bool),
    /// Mouse on the delete confirmation: `true` deletes, `false` keeps it.
    Delete(bool),
    /// Mouse on a question's answers: one taken, or `None` (cancel).
    Choose(Option<usize>),
    /// A group's arrow in the measurement tree: folds or unfolds it.
    ToggleGroup(ac2_proto::model::TraceOwner),
    /// A live curve's dot in the tree: shown / hidden (this app's display).
    ToggleMeasShown(MeasId),
    /// Ctrl+wheel on a pane: its level axis zooms by `factor` (> 1 in) about `about_db`.
    LevelZoom {
        pane: PaneKind,
        about_db: Option<f64>,
        factor: f64,
    },
    /// Shift+wheel on a pane: its level axis pans by `db` (positive: higher levels).
    LevelPan {
        pane: PaneKind,
        db: f64,
    },
    /// Mouse on the Leq windows dialog.
    Leq(LeqMsg),
    /// Mouse on the Settings view.
    Settings(SettingsMsg),
    /// Mouse on an impulse-response picture (the IR pane, the sweep pane's IR view).
    IrNav(ac2_scene::view::IrPane, IrNavMsg),
}

/// What the mouse does on the Leq windows dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeqMsg {
    Focus(crate::leq_dialog::Focus),
    /// ‹/› on a choice.
    Cycle(crate::leq_dialog::Focus, i32),
    /// Insert, or the plus at a section's heading: a window in the section of this focus,
    /// after its row (at the end from the heading).
    Add(crate::leq_dialog::Focus),
    /// − on a row or Delete: the window of this focus removed.
    Remove(crate::leq_dialog::Focus),
    /// "+ range…" at the band windows' heading: the range row opened.
    OpenRange,
    /// Add on the range row: one band window per band of it.
    AddRange,
    /// × on the range row: closed without adding.
    CloseRange,
    Submit,
    /// Closes the dialog without touching the stimulus.
    Cancel,
}

/// What the mouse does on the session dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionMsg {
    Focus(Row),
    /// ‹/› on the backend or device row.
    Cycle(Row, i32),
    /// The in-session box of an input or output.
    Toggle(Row),
    Role(Row, RoleKey),
    /// The mic name of an input.
    EditMic(Row),
    Detect,
    DetectConfirm,
    DetectCancel,
    Submit,
    /// Closes the dialog without touching the stimulus.
    Cancel,
}

/// What the mouse does on a dialog (the keyboard goes through [`Msg::Key`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormMsg {
    Focus(usize),
    /// ←/→ on field `.0`.
    Cycle(usize, i32),
    Submit,
    /// Closes the dialog without touching the stimulus.
    Cancel,
}

/// A run of a sweep measurement, armed (or arming): Enter plays it.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepPlan {
    pub meas: MeasId,
    /// The measurement's name.
    pub name: String,
    /// Its settings when armed: the generator is armed with its sweep, level and outputs.
    pub config: ac2_proto::model::SweepConfig,
}

/// Sweep measurements as this client runs them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SweepUi {
    /// The run armed (or arming): Enter plays it; cleared by a stop and once the run it
    /// started has ended.
    pub plan: Option<SweepPlan>,
    /// The run this client started, followed until it is stored or fails.
    pub run: Option<SweepId>,
    /// The last status of that run acted on.
    pub seen: Option<SweepStatus>,
    /// The sweep last selected: the distortion pane shows it while another kind of trace (or
    /// the live measurement) is selected; `None`: the newest.
    pub shown: Option<TraceId>,
    /// The lease is being given back after a finished sweep: its stop is no operator stop.
    pub releasing: bool,
    /// The dialog's sweep was submitted while a stop was in flight: it arms once the stop
    /// has landed.
    pub arm_after_stop: bool,
    /// The mirrored run's step (any client's run) and when this client first saw it, in
    /// `now_s`: the progress strip counts the time within a step from it.
    pub step_seen: Option<(SweepId, SweepStatus, f64)>,
    /// A finished run's result: once its data is in, the sweep pane's level axis frames it.
    pub fit: Option<TraceId>,
}

/// Everything the UI holds.
#[derive(Clone, Debug)]
pub struct AppState {
    pub conn: ConnState,
    pub mirror: Option<Arc<MirrorView>>,
    pub data: Option<Arc<DataSnapshot>>,
    /// Stored traces' data, as fetched.
    pub traces: BTreeMap<TraceId, (Arc<TraceData>, Arc<GridDef>)>,
    /// The transfer view's stored-trace display math, kept between frames.
    pub tf_display: ac2_scene::trace::DisplayCache,
    pub theme: ThemeName,
    /// What the scene builders get; `view.freq` follows `nav`.
    pub view: ViewState,
    pub nav: FreqNav,
    pub layout: Layout,
    pub selected: Option<MeasId>,
    /// The size of the panes' area as last drawn (the view writes it): Ctrl+N splits the
    /// focused pane along its longer side.
    pub pane_area: (f32, f32),
    /// The stored trace selected (list, V, the sweep pane's N): the trace keys change it
    /// instead of the pane's measurement, and a selected sweep is what the sweep pane shows.
    /// Selecting a measurement clears it, so whichever of the two was selected last is what
    /// the keys act on ([`AppState::keys_on_trace`]).
    pub selected_trace: Option<TraceId>,
    /// Measurements whose live curves this app hides, by name (as `ui.toml` keeps them):
    /// display only, they keep measuring.
    pub hidden_meas: BTreeSet<String>,
    /// Measurements (live curves, math channels) every transfer pane draws besides its own
    /// group, by name as hidden ones are (compare, C): an overlay, never an operand.
    pub compared_meas: BTreeSet<String>,
    /// Stored traces every transfer pane draws besides its own group (compare, C), whatever
    /// their owner. Ids are the daemon's: kept while the app runs, not in `ui.toml`.
    pub compared_traces: BTreeSet<TraceId>,
    /// Groups of the measurement tree folded in this app (their rows not listed).
    pub collapsed: BTreeSet<ac2_proto::model::TraceOwner>,
    pub edits: BTreeMap<MeasId, LiveEdit>,
    /// Peak hold per spectrum / RTA measurement, with the last folded-in `seq` and capture
    /// time.
    pub peaks: BTreeMap<MeasId, (u64, u64, PeakHold)>,
    /// Spectrograph history per spectrum / RTA measurement, kept while the spectrograph is
    /// shown.
    pub spectrographs: BTreeMap<MeasId, ac2_scene::spectrograph::SpectrographHistory>,
    pub stimulus: Stimulus,
    pub sweep: SweepUi,
    /// Remembered between runs (stimulus outputs per device).
    pub prefs: UiPrefs,
    /// `prefs` changed since the app last saved them.
    pub prefs_dirty: bool,
    /// The folder an export prompt starts in, and a relative path is relative to: the last
    /// export's, else the home directory. Never the working directory: started from a
    /// desktop icon, that is wherever the launcher left it.
    pub export_dir: Option<std::path::PathBuf>,
    /// Local time of day for wall times.
    pub local_zone: crate::scenes::LocalZone,
    /// The output device the stimulus outputs belong to (the open session's).
    stim_device: Option<String>,
    /// Band and observation X / Shift+X run the finder with.
    pub finder: FinderChoice,
    pub overlay: Overlay,
    /// How far the help overlay or the notification log is scrolled, points from the top
    /// (the view clamps it).
    pub help_scroll: f32,
    /// The scrolled window's visible height, points, as the view last measured it: a page.
    pub help_page: f32,
    /// Shown notifications, oldest first.
    pub toasts: Vec<Toast>,
    /// The pointer is over the toasts: none expires while the operator reads them.
    pub toasts_held: bool,
    toast_seq: u64,
    /// The last [`ac2_scene::toast::LOG_LEN`] notifications, oldest first.
    pub notices: std::collections::VecDeque<Notice>,
    pub now_s: f64,
    pub quit: bool,
    /// The window fills the screen (the app applies it).
    pub fullscreen: bool,
    /// The key that just opened a text overlay also arrives as text; drop that one char.
    swallow_text: Option<char>,
    /// Settings sent with the arm in flight; a change made before the arm is confirmed is
    /// sent once it is.
    armed_with: Option<GeneratorSettings>,
    /// A measurement this client just created: selected once the mirror lists it.
    pending_select: Option<MeasId>,
    /// Open the session dialog once the daemon's state shows no audio session (an embedded
    /// daemon on real audio starts without one).
    pub open_session_when_empty: bool,
    /// The daemon's backends and devices as last listed (channel names for the dialogs).
    pub devices: Option<Vec<ac2_proto::model::BackendInfo>>,
    /// When the device preview was last asked for.
    preview_sent_s: f64,
    /// The session the device list was last asked for on its own (for the input names of
    /// the always-on meters).
    devices_for: Option<ac2_proto::units::SessionEpoch>,
    /// Each SPL meter's Leq windows over time, from the `leq` frames received (with the
    /// last `seq` folded in) over the history rebuilt from the meter's log.
    pub leq_history: BTreeMap<MeasId, (u64, ac2_scene::leq::LeqHistory)>,
    /// Which log and windows each SPL meter's history was last rebuilt for.
    leq_logs: BTreeMap<MeasId, LeqLogSeen>,
    /// Number of the newest history rebuild asked for.
    leq_backfill_ask: u64,
    /// The newest over / recovered alarm of each meter already shown (`None`: none yet).
    leq_alarms_seen: BTreeMap<MeasId, Option<ac2_proto::model::LeqAlarm>>,
    /// Each SPL meter's displayed reading, held for its display period.
    pub spl_hold: BTreeMap<MeasId, ac2_scene::spl::SplHold>,
    /// The measurement each pane showed when the app last ran, by name, until the daemon's
    /// state is known.
    pending_pane_meas: BTreeMap<PaneId, String>,
    /// No layout was remembered: the one pane takes the kind of the first measurement once
    /// the daemon's state is known.
    pane_auto: bool,
    /// What the link was last asked to receive, and how often ([`crate::link_wants`]).
    pub(crate) link_wants: crate::link_wants::Sent,
    /// Spectrum / RTA measurements seen running, so a start is told from a run going on.
    spectrum_running: BTreeSet<MeasId>,
    /// This client's key, for the Settings view (`None`: it has none).
    pub client_key: Option<crate::settings::ClientKey>,
    /// The operator asked for the connect dialog (the app opens it).
    pub want_connect_dialog: bool,
    /// The selection moved by key: the tree brings its row into view (the view takes it).
    pub tree_reveal: bool,
    /// The Settings page last shown: the Settings key opens it again.
    settings_page_last: Page,
    /// Started spectrum / RTA measurements whose first frame fits the spectrum pane's level
    /// axis (as Shift+Home), with the frame shown when they started (a stopped run's last).
    spectrum_fit: BTreeMap<MeasId, Option<Arc<ac2_proto::Frame>>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(ThemeName::Dark, String::new())
    }
}

/// The text a plain key types, if any (letters, digits, `/`, `,`, `.`, space).
fn typed_char(c: &Chord) -> Option<char> {
    if c.command || c.alt {
        return None;
    }
    let name = c.key.symbol_or_name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Some(ch.to_ascii_lowercase()),
        _ if c.key == eframe::egui::Key::Space => Some(' '),
        _ => None,
    }
}

impl AppState {
    pub fn new(theme: ThemeName, target: String) -> Self {
        Self {
            conn: ConnState::Connecting { target },
            mirror: None,
            data: None,
            traces: BTreeMap::new(),
            theme,
            view: ViewState::default(),
            nav: FreqNav::new(FreqRange::default()),
            layout: Layout::default(),
            selected: None,
            pane_area: DEFAULT_PANE_AREA,
            selected_trace: None,
            edits: BTreeMap::new(),
            peaks: BTreeMap::new(),
            spectrographs: BTreeMap::new(),
            stimulus: Stimulus::default(),
            sweep: SweepUi::default(),
            prefs: UiPrefs::default(),
            prefs_dirty: false,
            export_dir: std::env::home_dir(),
            local_zone: crate::scenes::LocalZone::System,
            stim_device: None,
            finder: FinderChoice::default(),
            overlay: Overlay::None,
            help_scroll: 0.0,
            help_page: HELP_PAGE,
            toasts: Vec::new(),
            toasts_held: false,
            toast_seq: 0,
            notices: std::collections::VecDeque::new(),
            now_s: 0.0,
            quit: false,
            fullscreen: false,
            swallow_text: None,
            armed_with: None,
            pending_select: None,
            open_session_when_empty: false,
            devices: None,
            preview_sent_s: f64::NEG_INFINITY,
            devices_for: None,
            leq_history: BTreeMap::new(),
            tf_display: Default::default(),
            leq_logs: BTreeMap::new(),
            leq_backfill_ask: 0,
            leq_alarms_seen: BTreeMap::new(),
            spl_hold: BTreeMap::new(),
            pending_pane_meas: BTreeMap::new(),
            pane_auto: true,
            hidden_meas: BTreeSet::new(),
            compared_meas: BTreeSet::new(),
            compared_traces: BTreeSet::new(),
            collapsed: BTreeSet::new(),
            link_wants: crate::link_wants::Sent::default(),
            spectrum_running: BTreeSet::new(),
            spectrum_fit: BTreeMap::new(),
            client_key: None,
            want_connect_dialog: false,
            tree_reveal: false,
            settings_page_last: Page::Io,
        }
    }

    // ----- queries -----------------------------------------------------------------------

    pub fn update(&mut self, msg: Msg, keymap: &Keymap) -> Vec<Request> {
        let mut out = Vec::new();
        let before = self.meter_wants();
        let tick = matches!(msg, Msg::Tick { .. });
        self.update_inner(msg, keymap, &mut out);
        if tick {
            self.poll_settings(&mut out);
        }
        self.sync_meters(before, tick, &mut out);
        self.sync_session_watch(&mut out);
        if !tick {
            self.fit_focus();
            self.restore_pane_meas();
            self.prune_compared();
            self.remember_layout();
        }
        out
    }

    fn update_inner(&mut self, msg: Msg, keymap: &Keymap, out: &mut Vec<Request>) {
        let out = &mut *out;
        match msg {
            Msg::Key(chord) => self.key(chord, keymap, out),
            Msg::Text(t) => self.text(&t),
            Msg::Backspace => self.backspace(out),
            Msg::Wheel { rows } => self.wheel(rows, keymap),
            Msg::Command(c) => self.command(c, keymap, out),
            Msg::Conn(e) => self.conn_event(*e, keymap, out),
            Msg::Form(m) => self.form_msg(m, out),
            Msg::Session(m) => self.session_msg(m, out),
            Msg::Offer(create) => self.offer(create, out),
            Msg::NewLog(go) => self.new_log(go, out),
            Msg::Delete(go) => self.delete(go, out),
            Msg::LevelZoom {
                pane,
                about_db,
                factor,
            } => self.level_zoom(pane, about_db, factor),
            Msg::LevelPan { pane, db } => self.level_pan(pane, db),
            Msg::Leq(m) => self.leq_msg(m, out),
            Msg::Settings(m) => self.settings_msg(m, out),
            Msg::IrNav(p, m) => self.ir_nav(p, m),
            Msg::Tick { now_s, dt_s } => {
                let held = (now_s - self.now_s).max(0.0);
                self.now_s = now_s;
                self.nav.step(dt_s);
                self.view.freq = self.nav.current();
                if self.toasts_held {
                    for t in &mut self.toasts {
                        t.until_s += held;
                    }
                }
                let now = self.now_s;
                self.toasts.retain(|t| t.until_s > now);
            }
            Msg::ToastsHeld(held) => self.toasts_held = held,
            Msg::DismissToast(id) => {
                self.toasts.retain(|t| t.id != id);
                // What moved under the pointer is not what it rested on.
                self.toasts_held = false;
            }
            Msg::SelectMeas(id) => self.select_meas_row(id),
            Msg::SelectTrace(id) => {
                let id = (self.selected_trace != Some(id)).then_some(id);
                self.select_trace(id);
                self.reveal_trace();
                self.follow_selection_toast();
            }
            Msg::ToggleShown(id) => self.toggle_shown(id, out),
            Msg::ToggleGroup(g) => {
                if !self.collapsed.remove(&g) {
                    self.collapsed.insert(g);
                }
            }
            Msg::ToggleMeasShown(id) => self.toggle_meas_hidden(id, None),
            Msg::Choose(pick) => self.choose(pick, out),
            Msg::RenameTrace(id) => {
                if self.selected_trace != Some(id) {
                    self.select_trace(Some(id));
                    self.reveal_trace();
                }
                if let Ok(t) = self.trace_meta(id) {
                    self.prompt(PromptKind::TraceRename(id), t.edit.name.clone());
                }
            }
            Msg::FocusPane(p) => {
                self.focus_pane(p);
                // A click in a pane is about what it shows live.
                if self.pane_meas(p).is_some() {
                    self.selected_trace = None;
                }
            }
            Msg::PaneMenu(p) => {
                self.overlay = match self.overlay {
                    Overlay::PaneMenu(m) if m.pane == p => Overlay::None,
                    _ => self.pane_menu(p),
                };
            }
            Msg::PanePick(p, row) => self.pane_pick(p, row),
            Msg::Zoom { about_hz, factor } => {
                let t = self.nav.target.zoom(about_hz, factor);
                self.nav.set_target(t);
            }
            Msg::Pan { octaves } => {
                // Dragging follows the pointer directly: no spring lag.
                let t = self.nav.target.pan(octaves);
                self.nav = FreqNav::new(t);
                self.view.freq = t;
            }
            Msg::CursorAt(hz) => self.view.cursor_hz = hz,
            Msg::SpectrographCursor { hz, before_s } => {
                self.view.cursor_hz = Some(hz);
                self.view.spectrum.spectrograph.cursor_s = Some(before_s);
            }
            Msg::DistortionUnit(unit) => self.view.distortion.unit = unit,
            Msg::Legend(m) => {
                let l = &mut self.view.tf.legend;
                match m {
                    LegendMsg::Hover(h) => l.hover = h,
                    LegendMsg::Move { x, y } => l.move_to(x, y),
                    LegendMsg::Resize {
                        max_width,
                        max_height,
                    } => l.resize(max_width, max_height),
                    LegendMsg::Scroll { first } => l.first = first,
                }
            }
        }
    }

    /// Information: what a key or a reply did.
    fn toast(&mut self, text: impl Into<String>) {
        self.notify(Severity::Info, text.into(), Muted::Never);
    }

    /// A key refused or something missing, with what to do instead.
    fn warn(&mut self, text: impl Into<String>) {
        self.notify(Severity::Warning, text.into(), Muted::WithWarningToasts);
    }

    /// Something failed: a command, the link, the stimulus.
    fn fault(&mut self, text: impl Into<String>) {
        self.notify(Severity::Fault, text.into(), Muted::Never);
    }

    /// An Leq limit went over (`over`) or a window came back within it: an alarm the
    /// operator may be causing on purpose, so it goes quiet with the warnings.
    fn leq_alarm(&mut self, over: bool, text: String) {
        let severity = if over {
            Severity::Fault
        } else {
            Severity::Info
        };
        self.notify(severity, text, Muted::WithWarningToasts);
    }

    /// Shows `text` (unless `muted` by the Warning toasts setting) and logs it. The same
    /// message again replaces the one up (newest, its time restarted) rather than
    /// stacking copies, and counts up in the log.
    fn notify(&mut self, severity: Severity, text: String, muted: Muted) {
        let pops = match muted {
            Muted::Never => true,
            Muted::WithWarningToasts => self.prefs.warning_toasts,
        };
        if pops {
            self.toasts
                .retain(|t| !(t.severity == severity && t.text == text));
            if self.toasts.len() >= MAX_TOASTS {
                self.toasts.remove(0);
            }
            self.toast_seq += 1;
            self.toasts.push(Toast {
                id: self.toast_seq,
                until_s: self.now_s + ac2_scene::toast::duration_s(severity, &text),
                text: text.clone(),
                severity,
            });
        }
        match self.notices.back_mut() {
            Some(n) if n.severity == severity && n.text == text => {
                n.count += 1;
                n.at_s = self.now_s;
            }
            _ => {
                if self.notices.len() >= ac2_scene::toast::LOG_LEN {
                    self.notices.pop_front();
                }
                self.notices.push_back(Notice {
                    text,
                    severity,
                    at_s: self.now_s,
                    count: 1,
                });
            }
        }
    }
}

/// The stream a spectrum / RTA measurement's curve comes on; `None` for other kinds.
pub(crate) fn spectrum_stream(m: &Measurement) -> Option<Stream> {
    m.config
        .kind
        .stream()
        .filter(|_| m.config.kind.publishes_levels())
}

/// Fetched trace data takes the mirrored metadata, except the display smoothing: that one
/// describes the columns it was served with.
fn follow_meta(data: &mut Arc<TraceData>, m: &TraceMeta) {
    let mut want = m.clone();
    want.edit.smoothing = data.meta.edit.smoothing;
    if data.meta != want {
        Arc::make_mut(data).meta = want;
    }
}

/// Renames the input choices of a dialog (the device's channel names arrived).
fn relabel(f: &mut Form, names: &[(u16, String)]) {
    for field in &mut f.fields {
        if field.id == crate::forms::FieldId::Output {
            continue;
        }
        if let crate::forms::Value::Channel {
            channels, options, ..
        } = &mut field.value
        {
            for (c, o) in channels.iter().zip(options.iter_mut()) {
                if let Some((_, n)) = names.iter().find(|(x, _)| x == c) {
                    o.clone_from(n);
                }
            }
        }
    }
}

/// `press Shift+O (or Ctrl+K → Open audio session)`, from the keys actually bound.
/// The transfer pane's first-run guidance and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyHint {
    pub text: String,
    pub place: HintPlace,
}

/// Where the empty-pane hint is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintPlace {
    /// Centred over the empty plot: nothing else to see there.
    Centre,
    /// One line in the pane's title strip: the plot shows stored curves.
    Title,
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
