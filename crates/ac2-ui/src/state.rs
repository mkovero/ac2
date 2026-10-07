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
    SmoothingMode, State, SweepStatus, TraceData, TraceKind, TraceMeta,
};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{ClientId, Db, Dbfs, Hz, MeasId, Seconds, SweepId, TraceId};
use ac2_scene::spectrum::PeakHold;
use ac2_scene::stimulus::{Next as NextKey, Stimulus as NextStimulus};
use ac2_scene::theme::ThemeName;
use ac2_scene::trace::TraceKey;
use ac2_scene::view::{
    CoherencePlacement, DistortionUnit, FreqRange, IrMode, LeqStyle, PhaseView, SpectrumStyle,
    SplMode, SweepMode, ViewState,
};
use ac2_scene::{axis::Range, format};

use crate::anim::FreqNav;
use crate::cal_view::{CalAction, CalView};
use crate::conn::{ConnEvent, DataSnapshot, Request, StimEvent};
use crate::forms::{Form, FormKind, SweepPlan};
use crate::keys::{Chord, CommandId, Keymap, RESERVED, STOP_ANYWHERE, Scope};
use crate::leq_dialog::LeqDialog;
use crate::palette::Palette;
use crate::prefs::UiPrefs;
use crate::session_dialog::{RoleKey, Row, SessionDialog};
use crate::settings::{Page, Settings};

#[path = "state_display.rs"]
mod display;
pub use display::{DeleteTracePrompt, LEVEL_ZOOM_FACTOR, level_range};
#[path = "state_settings.rs"]
mod settings_impl;
pub use settings_impl::SettingsMsg;

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

    fn index(self) -> usize {
        self as usize
    }

    /// Whether the pane shows measurements of kind `k` (the IR pane shows the transfer
    /// pane's measurement).
    pub fn shows(self, k: &MeasKind) -> bool {
        match self {
            PaneKind::Transfer => k.publishes_tf(),
            // A math channel has no impulse response of its own.
            PaneKind::Ir => matches!(k, MeasKind::Transfer { .. }),
            PaneKind::Spectrum => k.publishes_levels(),
            PaneKind::Spl => matches!(k, MeasKind::Spl { .. }),
            // Sweep traces, not measurements.
            PaneKind::Distortion => false,
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
            Stream::Spec | Stream::Rta => PaneKind::Spectrum,
            Stream::Spl => PaneKind::Spl,
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
                MeasKind::Spl { .. } => Smoothable::No,
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
    pub pane: PaneKind,
    pub index: usize,
}

/// Which panes are shown and which has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub focus: PaneKind,
    pub shown: [bool; 5],
    /// Only the focused pane.
    pub maximized: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            focus: PaneKind::Transfer,
            // The sweep pane appears with the first sweep result.
            shown: [true, true, true, true, false],
            maximized: false,
        }
    }
}

impl Layout {
    pub fn is_shown(&self, p: PaneKind) -> bool {
        self.shown[p.index()]
    }

    /// Panes drawn now, in order.
    pub fn visible(&self) -> Vec<PaneKind> {
        if self.maximized {
            return vec![self.focus];
        }
        PaneKind::ALL
            .into_iter()
            .filter(|p| self.is_shown(*p))
            .collect()
    }
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

/// Display edits of a live trace (decision 8a: per-trace nudge; offset and polarity are
/// display only and go to the scene, which applies them).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveEdit {
    pub offset_db: f64,
    pub inverted: bool,
    pub nudge_s: f64,
}

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
}

impl Default for Stimulus {
    fn default() -> Self {
        Self {
            level: None,
            signal: Signal::Pink,
            outputs: vec![0],
            phase: StimPhase::Idle,
            arm_after_stop: false,
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
    /// The confirmation before a stored trace is deleted.
    DeleteTrace(Box<DeleteTracePrompt>),
}

impl Overlay {
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

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until_s: f64,
}

/// How long a toast stays up.
pub const TOAST_S: f64 = 4.0;
/// Coherence mask thresholds `B` cycles through (decision: blanking below γ²).
pub const COHERENCE_MASKS: [Option<f32>; 5] = [None, Some(0.3), Some(0.5), Some(0.7), Some(0.9)];
/// Pan step, octaves.
pub const PAN_OCTAVES: f64 = 1.0 / 3.0;
/// Zoom step: half an octave of span per key press each side.
pub const ZOOM_FACTOR: f64 = 1.5;
/// Delay nudge step.
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
    FocusPane(PaneKind),
    /// The pane title chip: opens (or closes) the pane's measurement list.
    PaneMenu(PaneKind),
    /// A measurement picked from a pane's list: the pane shows it, and it is selected.
    PaneShow(PaneKind, MeasId),
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
    /// Mouse on an open dialog.
    Form(FormMsg),
    /// Mouse on the session dialog.
    Session(SessionMsg),
    /// Mouse on the measurement offer: `true` creates, `false` skips.
    Offer(bool),
    /// Mouse on the new SPL log confirmation: `true` starts it, `false` keeps the log.
    NewLog(bool),
    /// Mouse on the delete confirmation: `true` deletes the trace, `false` keeps it.
    DeleteTrace(bool),
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
}

/// What the mouse does on the Leq windows dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeqMsg {
    Focus(crate::leq_dialog::Focus),
    /// ‹/› on a choice.
    Cycle(crate::leq_dialog::Focus, i32),
    /// Insert (a window after the focused one).
    Add,
    /// Delete (the focused window).
    Remove,
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

/// The sweep measurement as this client runs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SweepUi {
    /// The dialog's sweep: armed (or arming) with it, Enter plays it; cleared by a stop
    /// and once the run it started has ended.
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
    /// The last sweep this client played, as played (level and outputs as fired): Space on
    /// the sweep view arms it again.
    pub last: Option<SweepPlan>,
    /// The armed sweep is a re-sweep from the sweep view (not one the dialog set up).
    pub again: bool,
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
    /// The measurement each pane shows (keys act on it); a pane without a choice shows the
    /// selected measurement if it fits, else its first one.
    pub pane_meas: BTreeMap<PaneKind, MeasId>,
    /// The stored trace selected (list, V, the sweep pane's N): the trace keys change it
    /// instead of the pane's measurement, and a selected sweep is what the sweep pane shows.
    pub selected_trace: Option<TraceId>,
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
    /// How far the help overlay is scrolled, points from the top (the view clamps it).
    pub help_scroll: f32,
    /// The help overlay's visible height, points, as the view last measured it: a page.
    pub help_page: f32,
    pub toasts: Vec<Toast>,
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
    pending_pane_meas: BTreeMap<PaneKind, String>,
    /// What the link was last asked to receive, and how often ([`crate::link_wants`]).
    pub(crate) link_wants: crate::link_wants::Sent,
    /// Spectrum / RTA measurements seen running, so a start is told from a run going on.
    spectrum_running: BTreeSet<MeasId>,
    /// This client's key, for the Settings view (`None`: it has none).
    pub client_key: Option<crate::settings::ClientKey>,
    /// The operator asked for the connect dialog (the app opens it).
    pub want_connect_dialog: bool,
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
            pane_meas: BTreeMap::new(),
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
            link_wants: crate::link_wants::Sent::default(),
            spectrum_running: BTreeSet::new(),
            spectrum_fit: BTreeMap::new(),
            client_key: None,
            want_connect_dialog: false,
            settings_page_last: Page::Io,
        }
    }

    // ----- queries -----------------------------------------------------------------------

    /// The mirrored daemon state.
    pub fn daemon(&self) -> Option<&State> {
        self.mirror.as_ref().and_then(|m| m.state.as_deref())
    }

    pub fn measurements(&self) -> Vec<&Measurement> {
        let mut v: Vec<&Measurement> = self
            .daemon()
            .map(|s| s.measurements.iter().collect())
            .unwrap_or_default();
        v.sort_by_key(|m| m.id);
        v
    }

    pub fn meas(&self, id: MeasId) -> Option<&Measurement> {
        self.daemon()?.measurements.iter().find(|m| m.id == id)
    }

    pub fn selected_meas(&self) -> Option<&Measurement> {
        self.meas(self.selected?)
    }

    /// Measurements pane `p` can show, in list order.
    pub fn pane_candidates(&self, p: PaneKind) -> Vec<&Measurement> {
        self.measurements()
            .into_iter()
            .filter(|m| p.shows(&m.config.kind))
            .collect()
    }

    /// The measurement pane `p` shows: its own choice, else the selected measurement if it
    /// fits, else the first that fits.
    pub fn pane_meas(&self, p: PaneKind) -> Option<&Measurement> {
        let c = self.pane_candidates(p);
        let pick = |id: Option<MeasId>| id.and_then(|id| c.iter().find(|m| m.id == id).copied());
        pick(self.pane_meas.get(&p.owner()).copied())
            .or_else(|| pick(self.selected))
            .or_else(|| c.first().copied())
    }

    /// What a pane key (start/stop, reset) acts on: the measurement the focused pane shows,
    /// never one of another kind selected elsewhere (an SPL meter picked in the list must not
    /// stop when S is pressed on the transfer pane). Says what to create when there is none.
    fn focused_pane_meas(&mut self) -> Option<Measurement> {
        let p = self.layout.focus;
        if p == PaneKind::Distortion {
            let m = self.selected_meas().cloned();
            if m.is_none() {
                self.error("select a measurement first (N)");
            }
            return m;
        }
        let m = self.pane_meas(p).cloned();
        if m.is_none() {
            self.error(match p {
                PaneKind::Transfer | PaneKind::Ir => {
                    "no transfer measurement: Ctrl+K → New transfer measurement…"
                }
                PaneKind::Spectrum => "no spectrum or RTA: Ctrl+K → New spectrum / New RTA",
                _ => "no SPL meter: Ctrl+K → New SPL meter",
            });
        }
        m
    }

    /// Stored trace `id` as mirrored, or why not.
    fn trace_meta(&self, id: TraceId) -> Result<TraceMeta, String> {
        self.daemon()
            .and_then(|s| s.traces.iter().find(|t| t.id == id))
            .cloned()
            .ok_or_else(|| "that trace is gone (deleted meanwhile)".to_string())
    }

    /// The stored trace selected in the list, if it still exists.
    pub fn selected_trace_meta(&self) -> Option<&TraceMeta> {
        let id = self.selected_trace?;
        self.daemon()?.traces.iter().find(|t| t.id == id)
    }

    /// What K / Shift+K change now: the selected slot, else the spectrum pane's
    /// measurement when that pane has focus, else the transfer pane's.
    pub fn smooth_target(&self) -> Option<SmoothTarget> {
        if let Some(t) = self.selected_trace_meta() {
            return Some(SmoothTarget::Trace(t.clone()));
        }
        let pane = match self.layout.focus {
            PaneKind::Spectrum => PaneKind::Spectrum,
            _ => PaneKind::Transfer,
        };
        self.pane_meas(pane).map(|m| SmoothTarget::Meas(m.clone()))
    }

    /// A pane's title caption: the selected stored trace with its smoothing when its curve
    /// is drawn there, else the smoothing of the pane's measurement — `smoothing 1/6 oct`,
    /// `slot 3 (Main L S3): smoothing off`, `Sweep 2: smoothing off`; a selected target
    /// curve is named alone. Nothing for other curves smoothing does not apply to.
    pub fn smoothing_caption(&self, pane: PaneKind) -> Option<String> {
        if !matches!(pane, PaneKind::Transfer | PaneKind::Spectrum) {
            return None;
        }
        let t = match self.selected_trace_meta() {
            Some(t) if SmoothTarget::Trace(t.clone()).pane() == pane => {
                SmoothTarget::Trace(t.clone())
            }
            _ => SmoothTarget::Meas(self.pane_meas(pane)?.clone()),
        };
        if let SmoothTarget::Trace(_) = &t
            && t.kind() == Smoothable::No
        {
            return Some(t.label());
        }
        if !matches!(t.kind(), Smoothable::Transfer | Smoothable::Spectrum) {
            return None;
        }
        let c = t.caption();
        Some(match t {
            SmoothTarget::Trace(_) => format!("{}: {c}", t.label()),
            SmoothTarget::Meas(_) => c,
        })
    }

    /// The mic-curve note of a readout on `input` whose frame says the daemon applied a
    /// curve (`applied`) or not: `mic curve: MM1 34804 90°`, `mic curve off`,
    /// `no mic curve stored for MM1 34804` … (`ac2_scene::cal::curve_note`).
    pub fn curve_note(&self, input: u16, applied: bool) -> Option<String> {
        let s = self.daemon()?;
        ac2_scene::cal::curve_note(applied, &ac2_proto::cal::state_input_use(s, input).curve)
    }

    /// The mic-curve part of pane `pane`'s title caption: the selected stored trace's curve
    /// when the pane draws it, else the shown measurement's (from its newest frame), else
    /// the shown sweep's.
    pub fn mic_curve_caption(&self, pane: PaneKind) -> Option<String> {
        use ac2_proto::topic::Topic;
        let stored =
            |t: &TraceMeta| ac2_scene::trace::curve_note(t.mic.as_ref(), t.mic_curve.as_deref());
        match pane {
            PaneKind::Transfer | PaneKind::Spectrum => {
                if let Some(t) = self.selected_trace_meta()
                    && SmoothTarget::Trace(t.clone()).pane() == pane
                {
                    return stored(t);
                }
                let m = self.pane_meas(pane)?;
                let stream = m.config.kind.stream();
                let applied = self
                    .data
                    .as_ref()
                    .and_then(|d| d.latest.get(&Topic::Data { meas: m.id, stream }))
                    .is_some_and(|f| match &f.frame.data {
                        ac2_proto::FrameData::Tf(x) => x.meta.mic_curve,
                        ac2_proto::FrameData::Spec(x) => x.meta.mic_curve,
                        ac2_proto::FrameData::Rta(x) => x.meta.mic_curve,
                        _ => false,
                    });
                meas_input(&m.config.kind).and_then(|i| self.curve_note(i, applied))
            }
            PaneKind::Distortion => self.shown_sweep().and_then(|(d, _)| stored(&d.meta)),
            PaneKind::Ir | PaneKind::Spl => None,
        }
    }

    /// The pane's title caption: smoothing and mic curve, `smoothing 1/6 oct · mic curve:
    /// MM1 34804 90°`.
    pub fn pane_caption(&self, pane: PaneKind) -> Option<String> {
        self.pane_caption_variants(pane).into_iter().next()
    }

    /// The pane's title caption from the longest to the shortest, for a narrow title: all
    /// of it, then without the mic curve, then the selected stored trace's name alone (the
    /// one thing the title must keep: which curve the keys act on).
    pub fn pane_caption_variants(&self, pane: PaneKind) -> Vec<String> {
        let smoothing = self.smoothing_caption(pane);
        let curve = self.mic_curve_caption(pane);
        let trace = self
            .selected_trace_meta()
            .filter(|t| {
                matches!(pane, PaneKind::Transfer | PaneKind::Spectrum)
                    && SmoothTarget::Trace((*t).clone()).pane() == pane
            })
            .map(trace_label);
        let mut v: Vec<String> = Vec::new();
        let parts: Vec<&String> = [&smoothing, &curve].into_iter().flatten().collect();
        if !parts.is_empty() {
            v.push(
                parts
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
        v.extend(smoothing);
        v.extend(trace);
        v.dedup();
        v
    }

    pub fn edit(&self, id: MeasId) -> LiveEdit {
        self.edits.get(&id).copied().unwrap_or_default()
    }

    /// Stored trace metadata as mirrored (edits, slots and visibility are the daemon's).
    pub fn stored_traces(&self) -> Vec<&TraceMeta> {
        let mut v: Vec<&TraceMeta> = self
            .daemon()
            .map(|s| s.traces.iter().collect())
            .unwrap_or_default();
        v.sort_by_key(|t| (t.edit.order, t.id));
        v
    }

    /// Every stored trace in the list's order (slotted by slot, then the rest oldest first):
    /// the order V / Shift+V step through.
    pub fn trace_list(&self) -> Vec<&TraceMeta> {
        let mut v = self.stored_traces();
        v.sort_by_key(|t| ac2_scene::trace_list::sort_key(t));
        v
    }

    /// The sidebar's rows of stored traces.
    pub fn trace_rows(&self) -> Vec<ac2_scene::trace_list::TraceRow> {
        let items: Vec<ac2_scene::trace_list::TraceItem<'_>> = self
            .stored_traces()
            .into_iter()
            .map(|meta| ac2_scene::trace_list::TraceItem {
                meta,
                has_data: self.traces.contains_key(&meta.id),
            })
            .collect();
        ac2_scene::trace_list::trace_rows(&items, self.selected_trace)
    }

    /// The trace in each slot 1…9 (index 0 = slot 1).
    pub fn slots(&self) -> [Option<&TraceMeta>; 9] {
        let mut out = [None; 9];
        for t in self
            .daemon()
            .map(|s| s.traces.as_slice())
            .unwrap_or_default()
        {
            if let Some(n) = t.edit.slot.filter(|n| (1..=9).contains(n)) {
                out[usize::from(n - 1)] = Some(t);
            }
        }
        out
    }

    /// Shown stored traces on the transfer pane, slotted first (by slot), then by order.
    fn shown_transfer_traces(&self) -> Vec<&TraceMeta> {
        let mut v: Vec<&TraceMeta> = self
            .stored_traces()
            .into_iter()
            .filter(|t| {
                t.edit.visible
                    && matches!(
                        t.kind,
                        TraceKind::Transfer | TraceKind::Target | TraceKind::Sweep
                    )
            })
            .collect();
        v.sort_by_key(|t| (t.edit.slot.unwrap_or(u8::MAX), t.edit.order, t.id));
        v
    }

    /// This connection's identity. The mirror's is authoritative: it belongs to the daemon
    /// incarnation the shown state comes from, and a restarted daemon binds a new one (the
    /// id from the connect would make another client's lease look like ours, or ours like
    /// another's).
    pub fn my_client_id(&self) -> Option<&ClientId> {
        let ConnState::Connected { client_id, .. } = &self.conn else {
            return None;
        };
        match &self.mirror {
            Some(m) if m.incarnation.is_some() => m.client_id.as_ref(),
            _ => Some(client_id),
        }
    }

    /// The daemon's open audio session.
    pub fn open_session(&self) -> Option<&ac2_proto::model::OpenSession> {
        self.daemon().and_then(|s| s.session.open.as_ref())
    }

    /// What the transfer pane says when there is nothing to measure yet: no audio session,
    /// or a session without measurements. `None` once there is something (or no daemon).
    /// Over stored curves it moves to the pane's title strip, out of their way.
    pub fn empty_hint(&self, keymap: &Keymap) -> Option<EmptyHint> {
        if !self.connected() {
            return None;
        }
        let st = self.daemon()?;
        let text = if st.session.open.is_none() {
            format!("No audio session — {}", open_session_hint(keymap))
        } else if st.measurements.is_empty() {
            let palette = keymap
                .chords(CommandId::Palette, Scope::Global)
                .first()
                .map_or_else(|| "Command palette".to_owned(), |c| c.label());
            format!(
                "No measurements — {palette} → New transfer measurement… (or New spectrum, RTA, SPL meter)"
            )
        } else {
            return None;
        };
        let place = if self.transfer_shows_stored() {
            HintPlace::Title
        } else {
            HintPlace::Centre
        };
        Some(EmptyHint { text, place })
    }

    /// Whether the transfer pane draws any stored curve (a shown capture, target or sweep
    /// whose data has arrived).
    pub fn transfer_shows_stored(&self) -> bool {
        self.traces.values().any(|(t, _)| on_transfer_pane(&t.meta))
    }

    pub fn connected(&self) -> bool {
        matches!(self.conn, ConnState::Connected { .. })
    }

    /// Keys go to the focused pane's scope.
    pub fn scope(&self) -> Scope {
        self.layout.focus.scope()
    }

    /// Navigation still moving: the UI keeps repainting until it settles.
    pub fn animating(&self) -> bool {
        !self.nav.settled()
    }

    /// Generator ceiling from the daemon.
    pub fn ceiling(&self) -> Option<Dbfs> {
        self.daemon().map(|s| s.generator.ceiling)
    }

    /// Something may be emitting or armed: ours in flight, or the mirrored generator.
    /// A window drawn over the panes (behind a backdrop) is open: it owns the keyboard and
    /// the mouse. The delay candidates and a pane's measurement list stay out of the way.
    pub fn window_over_panes(&self) -> bool {
        !matches!(
            self.overlay,
            Overlay::None | Overlay::DelayPick(_) | Overlay::PaneMenu(_)
        )
    }

    pub fn stimulus_live(&self) -> bool {
        self.stimulus.phase != StimPhase::Idle
            || self
                .daemon()
                .is_some_and(|s| s.generator.armed || s.generator.firing)
    }

    // ----- reducer -----------------------------------------------------------------------

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
            self.restore_pane_meas();
            self.remember_layout();
        }
        out
    }

    /// Once the daemon's state is known, each pane shows the measurement it showed when the
    /// app last ran, if one of that name (and kind) still exists; else the pane's usual
    /// choice, quietly.
    fn restore_pane_meas(&mut self) {
        if self.pending_pane_meas.is_empty() || self.daemon().is_none() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_pane_meas);
        for (pane, name) in pending {
            let id = self
                .measurements()
                .iter()
                .find(|m| m.config.name == name && PaneKind::for_kind(&m.config.kind) == pane)
                .map(|m| m.id);
            if let Some(id) = id {
                self.pane_meas.insert(pane, id);
                if self.layout.focus.owner() == pane {
                    self.select(id);
                }
            }
        }
    }

    /// The layout as it is now, as the preferences keep it.
    pub fn layout_prefs(&self) -> crate::prefs::LayoutPrefs {
        let mut measurements = BTreeMap::new();
        for p in [PaneKind::Transfer, PaneKind::Spectrum, PaneKind::Spl] {
            // Unknown until the daemon's state is (and while a remembered one waits for
            // it): keep what the preferences say.
            let name = match (self.pending_pane_meas.get(&p), self.daemon()) {
                (Some(n), _) => Some(n.clone()),
                (None, None) => self.prefs.layout.measurements.get(&p).cloned(),
                (None, Some(_)) => self.pane_meas(p).map(|m| m.config.name.clone()),
            };
            if let Some(n) = name {
                measurements.insert(p, n);
            }
        }
        crate::prefs::LayoutPrefs {
            focus: self.layout.focus,
            maximized: self.layout.maximized,
            fullscreen: self.fullscreen,
            spl_view: self.view.spl.mode,
            spectrum_view: self.view.spectrum.mode,
            sweep_view: self.view.distortion.mode,
            ir_mode: self.view.ir.mode,
            distortion_unit: self.view.distortion.unit,
            measurements,
        }
    }

    /// Saves the layout and the level axes with the preferences when they changed.
    fn remember_layout(&mut self) {
        let now = self.layout_prefs();
        if now != self.prefs.layout {
            self.prefs.layout = now;
            self.prefs_dirty = true;
        }
        let levels = crate::prefs::LevelPrefs::of(&self.view);
        if levels != self.prefs.levels {
            self.prefs.levels = levels;
            self.prefs_dirty = true;
        }
    }

    /// What needs metering: the session's input meters while a session is open (the
    /// sidebar shows them all the time) or a dialog meters inputs, and a device preview (the
    /// session dialog on a device the session does not capture).
    fn meter_wants(
        &self,
    ) -> (
        bool,
        Option<(ac2_proto::model::BackendKind, ac2_proto::model::DeviceId)>,
    ) {
        let session = self.open_session().is_some();
        match &self.overlay {
            _ if self.overlay.session().is_some() => (
                true,
                self.overlay.session().and_then(|d| d.preview_target()),
            ),
            Overlay::Form(_) => (true, None),
            _ => (session, None),
        }
    }

    /// Follows what the always-on parts of the window need from the mirror: the device
    /// list once per session (the inputs' channel names), and when the running sweep's
    /// step began.
    fn sync_session_watch(&mut self, out: &mut Vec<Request>) {
        let epoch = self
            .daemon()
            .filter(|s| s.session.open.is_some())
            .map(|s| s.session.epoch);
        if let Some(e) = epoch
            && self.devices.is_none()
            && self.devices_for != Some(e)
            && self.connected()
        {
            self.devices_for = Some(e);
            if !matches!(self.overlay, Overlay::Settings(_) | Overlay::Form(_)) {
                out.push(Request::Devices);
            }
        }
        let run = self
            .daemon()
            .and_then(|s| s.sweep.as_ref())
            .filter(|r| r.active())
            .map(|r| (r.id, r.status.clone()));
        match run {
            None => self.sweep.step_seen = None,
            Some((id, status)) => {
                let same = self
                    .sweep
                    .step_seen
                    .as_ref()
                    .is_some_and(|(i, s, _)| *i == id && *s == status);
                if !same {
                    self.sweep.step_seen = Some((id, status, self.now_s));
                }
            }
        }
    }

    /// Takes the preferences read at startup, and the view choices they hold: the layout as
    /// last left (the measurements each pane showed come back once the daemon's state is
    /// known). Nothing here arms or plays anything.
    pub fn set_prefs(&mut self, prefs: UiPrefs) {
        self.view.spl.layout = prefs.leq;
        let l = &prefs.layout;
        self.layout.shown[l.focus.index()] = true;
        self.layout.focus = l.focus;
        self.layout.maximized = l.maximized;
        self.fullscreen = l.fullscreen;
        self.view.spl.mode = l.spl_view;
        self.view.spectrum.mode = l.spectrum_view;
        self.view.distortion.mode = l.sweep_view;
        self.view.ir.mode = l.ir_mode;
        self.view.distortion.unit = l.distortion_unit;
        // Before any frame: a spectrum that starts still fits its axis on its first one.
        prefs.levels.apply(&mut self.view);
        if let Some(s) = prefs.spectrograph_span_s {
            self.view.spectrum.spectrograph.span_s = s;
        }
        self.pending_pane_meas = l.measurements.clone();
        self.prefs = prefs;
    }

    /// W: the split layout → the focused pane alone → the focused pane full screen (the
    /// window too) → the split layout again.
    fn cycle_layout(&mut self) {
        if !self.layout.maximized {
            self.layout.maximized = true;
        } else if !self.fullscreen {
            self.fullscreen = true;
        } else {
            self.layout.maximized = false;
            self.fullscreen = false;
        }
    }

    /// The stage view: the focused pane alone with the window full screen, so the screen
    /// holds only the pane's picture (no top bar, list or pane title; the SPL meter and its
    /// Leq windows read across a room). Whenever a stimulus may be sounding or an operation
    /// runs, the top bar comes back: what is driving the speakers is never hidden.
    pub fn stage_view(&self) -> bool {
        self.fullscreen
            && self.layout.maximized
            && self.stimulus.phase == StimPhase::Idle
            && !self.daemon().is_some_and(|d| d.generator.firing)
            && self.operation().is_none()
    }

    /// Whether the panes show key hints now: on in the preferences, and never in the stage
    /// view (the audience sees the windows alone).
    pub fn key_hints_shown(&self) -> bool {
        self.prefs.key_hints && !self.stage_view()
    }

    /// The hint line of `pane`: its most used keys as bound in `keymap`, written in `style`,
    /// then the help key. Only the focused pane has one, and only while hints are shown.
    /// Commands that do nothing in the pane's present view are left out (the sweep pane's
    /// dB / % while it shows the IR, its IR mode while it shows distortion).
    pub fn key_hint_line(
        &self,
        keymap: &Keymap,
        pane: PaneKind,
        style: crate::keys::LabelStyle,
    ) -> Option<Vec<crate::hints::KeyHint>> {
        if !self.key_hints_shown() || self.layout.focus != pane {
            return None;
        }
        Some(self.pane_hints(keymap, pane, style))
    }

    /// Every hint of `pane` (the title's tooltip lists them whether or not the line shows).
    pub fn pane_hints(
        &self,
        keymap: &Keymap,
        pane: PaneKind,
        style: crate::keys::LabelStyle,
    ) -> Vec<crate::hints::KeyHint> {
        let mode = self.view.distortion.mode;
        crate::hints::line(keymap, pane.scope(), style, |c| {
            pane == PaneKind::Distortion
                && match c {
                    CommandId::DistortionUnit => mode != SweepMode::Response,
                    CommandId::IrMode => mode != SweepMode::Ir,
                    _ => false,
                }
        })
    }

    /// The multi-step operation running on the daemon (a set of sweeps), as the progress
    /// strip shows it.
    pub fn operation(&self) -> Option<ac2_scene::progress::Progress> {
        let run = self.daemon()?.sweep.as_ref()?;
        let since = self
            .sweep
            .step_seen
            .as_ref()
            .filter(|(i, s, _)| *i == run.id && *s == run.status)
            .map_or(0.0, |(_, _, t)| self.now_s - t);
        ac2_scene::progress::sweep(run, since)
    }

    /// Every input the open session captures, labelled by name and role, with its meter
    /// and what the running sweep (else the selected measurement) uses it as.
    pub fn session_inputs(&self) -> Vec<ac2_scene::meter::InputRow> {
        use ac2_scene::meter::{InputRole, InputRow, InputUse, MeterReading, input_label};
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let meters = if self.overlay.session().is_some() {
            // The dialog's meters may be another device's preview.
            BTreeMap::new()
        } else {
            self.input_meters()
        };
        let device_names = self
            .devices
            .iter()
            .flatten()
            .filter(|b| b.kind == o.backend)
            .flat_map(|b| &b.devices)
            .find(|d| d.id == o.input_device)
            .and_then(|d| d.input.as_ref())
            .and_then(|i| i.channel_names.clone());
        let ms = self.measurements();
        let loopback = o.config.loopback.map(|l| l.input);
        let run = self
            .daemon()
            .and_then(|s| s.sweep.as_ref())
            .filter(|r| r.active());
        let uses: Vec<(u16, InputUse)> = match (run, self.selected_meas()) {
            (Some(r), _) => vec![
                (r.reference_input, InputUse::Reference),
                (r.measurement_input, InputUse::Measurement),
            ],
            (None, Some(m)) => match &m.config.kind {
                MeasKind::Transfer { config } => vec![
                    (config.reference_input, InputUse::Reference),
                    (config.measurement_input, InputUse::Measurement),
                ],
                MeasKind::Spectrum { config } => vec![(config.input, InputUse::Measurement)],
                MeasKind::Rta { config } => vec![(config.input, InputUse::Measurement)],
                MeasKind::Spl { config } => vec![(config.input, InputUse::Measurement)],
                // Every live operand's inputs.
                MeasKind::Math { config } => ms
                    .iter()
                    .filter(|x| config.expr.names(Operand::Meas { meas: x.id }))
                    .flat_map(|x| match &x.config.kind {
                        MeasKind::Transfer { config } => vec![
                            (config.reference_input, InputUse::Reference),
                            (config.measurement_input, InputUse::Measurement),
                        ],
                        MeasKind::Spectrum { config } => {
                            vec![(config.input, InputUse::Measurement)]
                        }
                        MeasKind::Rta { config } => vec![(config.input, InputUse::Measurement)],
                        _ => Vec::new(),
                    })
                    .collect(),
            },
            (None, None) => Vec::new(),
        };
        o.config
            .input_channels
            .iter()
            .map(|&c| {
                let mic = self.input_setup(c).mic;
                let dev = device_names
                    .as_ref()
                    .and_then(|n| n.get(usize::from(c)).cloned());
                let name = ac2_scene::meter::input_name(c, mic.as_deref(), dev.as_deref());
                let is_ref = loopback == Some(c)
                    || ms.iter().any(|m| {
                        matches!(&m.config.kind, MeasKind::Transfer { config }
                            if config.reference_input == c)
                    });
                let is_mic = mic.is_some()
                    || ms.iter().any(|m| {
                        matches!(&m.config.kind, MeasKind::Transfer { config }
                            if config.measurement_input == c)
                    });
                let role = if is_ref {
                    Some(InputRole::Reference)
                } else if is_mic {
                    Some(InputRole::Mic)
                } else {
                    None
                };
                let curve = self.daemon().and_then(|s| {
                    ac2_scene::cal::curve_short(&ac2_proto::cal::state_input_use(s, c).curve)
                });
                InputRow {
                    channel: c,
                    label: input_label(c, &name, curve.as_deref(), role),
                    used: uses.iter().find(|(i, _)| *i == c).map(|(_, u)| *u),
                    reading: meters.get(&c).cloned().unwrap_or_else(MeterReading::none),
                }
            })
            .collect()
    }

    /// Subscribes, opens, renews and closes what [`Self::meter_wants`] changed to.
    fn sync_meters(
        &mut self,
        before: (
            bool,
            Option<(ac2_proto::model::BackendKind, ac2_proto::model::DeviceId)>,
        ),
        tick: bool,
        out: &mut Vec<Request>,
    ) {
        let after = self.meter_wants();
        if before.0 != after.0 {
            out.push(Request::Meters(after.0));
        }
        let renew = tick && self.now_s - self.preview_sent_s >= PREVIEW_RENEW_S;
        match (&before.1, &after.1) {
            (_, Some((backend, device))) if before.1 != after.1 || renew => {
                out.push(Request::Preview {
                    backend: *backend,
                    device: device.clone(),
                });
                self.preview_sent_s = self.now_s;
            }
            (Some(_), None) => out.push(Request::PreviewStop),
            _ => {}
        }
    }

    /// Input meters of the session dialog's device (its preview, or the session's own
    /// meters when the session captures it) or, for the measurement dialogs, of the
    /// session: device input → reading. Stale frames read as nothing.
    pub fn input_meters(&self) -> BTreeMap<u16, ac2_scene::meter::MeterReading> {
        use ac2_proto::FrameData;
        use ac2_proto::topic::Topic;
        let mut out = BTreeMap::new();
        let Some(d) = &self.data else {
            return out;
        };
        let from_preview = self.overlay.session().and_then(|s| s.preview_target());
        let topic = if from_preview.is_some() {
            Topic::PreviewLevels
        } else {
            Topic::SessionLevels
        };
        let Some(tf) = d.latest.get(&topic).filter(|f| !f.stale) else {
            return out;
        };
        let (channels, peak, rms, clip) = match &tf.frame.data {
            FrameData::SessionLevels(f) => (&f.meta.channels, &f.peak, &f.rms, &f.clip),
            FrameData::PreviewLevels(f) => {
                if from_preview.as_ref() != Some(&(f.meta.backend, f.meta.device.clone())) {
                    return out;
                }
                (&f.meta.channels, &f.peak, &f.rms, &f.clip)
            }
            _ => return out,
        };
        for (i, c) in channels.iter().enumerate() {
            let (Some(p), Some(r), Some(k)) = (peak.get(i), rms.get(i), clip.get(i)) else {
                continue;
            };
            out.insert(
                *c,
                ac2_scene::meter::MeterReading::new(
                    *p,
                    *r,
                    *k != ac2_proto::frame::ClipFlags::NONE,
                ),
            );
        }
        out
    }

    /// The names of the open session's outputs: the device's channel name, else `Output N`.
    pub fn session_output_names(&self) -> Vec<(u16, String)> {
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let device_names = self
            .devices
            .iter()
            .flatten()
            .filter(|b| b.kind == o.backend)
            .flat_map(|b| &b.devices)
            .find(|d| d.id == o.output_device)
            .and_then(|d| d.output.as_ref())
            .and_then(|i| i.channel_names.clone());
        (0..o.config.output_channels)
            .map(|c| {
                let name = device_names
                    .as_ref()
                    .and_then(|n| n.get(usize::from(c)).cloned())
                    .unwrap_or_else(|| format!("Output {}", c + 1));
                (c, name)
            })
            .collect()
    }

    /// The names of the open session's inputs, as the dialogs show them: mic name, else the
    /// device's channel name, else `Input N`.
    pub fn session_input_names(&self) -> Vec<(u16, String)> {
        self.session_input_labels()
            .into_iter()
            .map(|(c, name)| (c, ac2_scene::meter::channel_choice(c, &name)))
            .collect()
    }

    /// The open session's inputs by name alone (mic, device channel name or `Input N`).
    pub fn session_input_labels(&self) -> Vec<(u16, String)> {
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let device_names = self
            .devices
            .iter()
            .flatten()
            .filter(|b| b.kind == o.backend)
            .flat_map(|b| &b.devices)
            .find(|d| d.id == o.input_device)
            .and_then(|d| d.input.as_ref())
            .and_then(|i| i.channel_names.clone());
        o.config
            .input_channels
            .iter()
            .map(|&c| {
                let mic = self.input_setup(c).mic;
                let dev = device_names
                    .as_ref()
                    .and_then(|n| n.get(usize::from(c)).cloned());
                (
                    c,
                    ac2_scene::meter::input_name(c, mic.as_deref(), dev.as_deref()),
                )
            })
            .collect()
    }

    fn update_inner(&mut self, msg: Msg, keymap: &Keymap, out: &mut Vec<Request>) {
        let out = &mut *out;
        match msg {
            Msg::Key(chord) => self.key(chord, keymap, out),
            Msg::Text(t) => self.text(&t),
            Msg::Backspace => match &mut self.overlay {
                Overlay::Palette(p) => p.backspace(),
                Overlay::Form(f) => f.backspace(),
                // Not typing: Backspace deletes where Delete deletes.
                Overlay::Settings(_) => self.settings_backspace(out),
                Overlay::DeleteTrace(_) => self.delete_trace(false, out),
                Overlay::Prompt(p) => {
                    p.text.pop();
                    p.error = None;
                }
                _ => {}
            },
            Msg::Wheel { rows } => self.wheel(rows, keymap),
            Msg::Command(c) => self.command(c, keymap, out),
            Msg::Conn(e) => self.conn_event(*e, keymap, out),
            Msg::Form(m) => self.form_msg(m, out),
            Msg::Session(m) => self.session_msg(m, out),
            Msg::Offer(create) => self.offer(create, out),
            Msg::NewLog(go) => self.new_log(go, out),
            Msg::DeleteTrace(go) => self.delete_trace(go, out),
            Msg::LevelZoom {
                pane,
                about_db,
                factor,
            } => self.level_zoom(pane, about_db, factor),
            Msg::LevelPan { pane, db } => self.level_pan(pane, db),
            Msg::Leq(m) => self.leq_msg(m, out),
            Msg::Settings(m) => self.settings_msg(m, out),
            Msg::Tick { now_s, dt_s } => {
                self.now_s = now_s;
                self.nav.step(dt_s);
                self.view.freq = self.nav.current();
                let now = self.now_s;
                self.toasts.retain(|t| t.until_s > now);
            }
            Msg::SelectMeas(id) => {
                self.select(id);
                self.reveal_meas(id);
            }
            Msg::SelectTrace(id) => {
                let id = (self.selected_trace != Some(id)).then_some(id);
                self.select_trace(id);
                self.reveal_trace();
            }
            Msg::ToggleShown(id) => self.toggle_shown(id, out),
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
                self.focus(p);
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
            Msg::PaneShow(p, id) => self.pane_show(p, id),
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
        }
    }

    /// The wheel moves a list window's highlight; it never reaches what is behind it.
    fn wheel(&mut self, rows: i32, keymap: &Keymap) {
        let scope = self.layout.focus.scope();
        match &mut self.overlay {
            Overlay::Palette(p) => {
                let n = p.entries(keymap, scope).len();
                p.move_by(rows, n);
            }
            Overlay::PaneMenu(m) => {
                let mut m = *m;
                let n = self.pane_candidates(m.pane).len();
                if n > 0 {
                    m.index = (m.index as i64 + i64::from(rows)).clamp(0, n as i64 - 1) as usize;
                    self.overlay = Overlay::PaneMenu(m);
                }
            }
            _ => {}
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            error: false,
            until_s: self.now_s + TOAST_S,
        });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            error: true,
            until_s: self.now_s + TOAST_S * 1.5,
        });
    }

    fn key(&mut self, chord: Chord, keymap: &Keymap, out: &mut Vec<Request>) {
        use eframe::egui::Key;
        let swallow = self.swallow_text.take();
        // The stop that works from anywhere, before any window sees the key.
        if chord == STOP_ANYWHERE {
            self.command(CommandId::StopAnywhere, keymap, out);
            return;
        }
        // An open window owns Esc: it closes the topmost window only. With nothing open Esc
        // stops the stimulus and hands the keys back from a selected slot to the live
        // measurement.
        if chord == Chord::key(Key::Escape) {
            if self.overlay == Overlay::None {
                self.command(CommandId::StimulusStop, keymap, out);
                self.selected_trace = None;
            } else {
                self.close_overlay(out);
            }
            return;
        }
        let plain = !(chord.command || chord.alt);
        match &mut self.overlay {
            Overlay::Help => {
                let page = self.help_page.max(HELP_LINE);
                match chord.key {
                    Key::ArrowDown | Key::ArrowUp if plain => {
                        let d = if chord.key == Key::ArrowDown {
                            HELP_LINE
                        } else {
                            -HELP_LINE
                        };
                        self.help_scroll = (self.help_scroll + d).max(0.0);
                    }
                    Key::PageDown if plain => self.help_scroll += page,
                    Key::PageUp if plain => self.help_scroll = (self.help_scroll - page).max(0.0),
                    Key::Home if plain => self.help_scroll = 0.0,
                    // The view clamps it to the end.
                    Key::End if plain => self.help_scroll = f32::MAX,
                    Key::Enter => self.overlay = Overlay::None,
                    // Other keys keep working with the keys shown (try them while reading),
                    // except the stimulus's.
                    _ => self.overlay_fallthrough(chord, keymap, out),
                }
                return;
            }
            Overlay::Palette(p) => {
                let scope = self.layout.focus.scope();
                let n = p.entries(keymap, scope).len();
                let page = crate::palette::PALETTE_ROWS as i32;
                match chord.key {
                    Key::ArrowDown | Key::ArrowUp => {
                        p.move_by(if chord.key == Key::ArrowDown { 1 } else { -1 }, n);
                    }
                    Key::PageDown => p.move_by(page, n),
                    Key::PageUp => p.move_by(-page, n),
                    Key::Home => p.move_by(-(n as i32), n),
                    Key::End => p.move_by(n as i32, n),
                    Key::Enter => {
                        let c = p.chosen(keymap, scope);
                        self.overlay = Overlay::None;
                        if let Some(c) = c {
                            self.command(c, keymap, out);
                        }
                    }
                    _ if keymap.lookup(Scope::Global, chord) == Some(CommandId::Palette) => {
                        self.overlay = Overlay::None;
                    }
                    _ => self.swallow_text = swallow,
                }
                return;
            }
            Overlay::Prompt(_) => {
                if chord.key == Key::Enter {
                    self.apply_prompt(out);
                } else {
                    self.swallow_text = swallow;
                }
                return;
            }
            Overlay::Settings(_) => {
                self.settings_key(chord, swallow, keymap, out);
                return;
            }
            Overlay::Offer(_) => {
                if chord.key == Key::Enter {
                    self.offer(true, out);
                } else if matches!(chord.key, Key::Backspace | Key::N) {
                    self.offer(false, out);
                }
                return;
            }
            Overlay::NewLog(_) => {
                if chord.key == Key::Enter {
                    self.new_log(true, out);
                } else if matches!(chord.key, Key::Backspace | Key::N) {
                    self.new_log(false, out);
                }
                return;
            }
            // Delete twice deletes, as in the calibrations view.
            Overlay::DeleteTrace(_) => {
                if matches!(chord.key, Key::Enter | Key::Delete) {
                    self.delete_trace(true, out);
                } else if matches!(chord.key, Key::Backspace | Key::N) {
                    self.delete_trace(false, out);
                }
                return;
            }
            Overlay::Form(f) => {
                match chord.key {
                    Key::Enter => self.submit_form(out),
                    Key::ArrowUp => f.move_focus(-1),
                    Key::Tab if chord.shift => f.move_focus(-1),
                    Key::ArrowDown | Key::Tab => f.move_focus(1),
                    Key::ArrowLeft => f.cycle(-1),
                    Key::ArrowRight => f.cycle(1),
                    Key::A if chord.command => f.select_all(),
                    _ => self.swallow_text = swallow,
                }
                return;
            }
            Overlay::DelayPick(choice) => {
                let n = choice.rows().len();
                if matches!(chord.key, Key::ArrowDown | Key::ArrowUp) && plain && !chord.shift {
                    if n > 0 {
                        let d = if chord.key == Key::ArrowDown {
                            1
                        } else {
                            n - 1
                        };
                        choice.selected = (choice.selected + d) % n;
                    }
                    return;
                }
                let index = match chord {
                    c if c == Chord::key(Key::Num1) => Some(0u8),
                    c if c == Chord::key(Key::Num2) => Some(1),
                    c if c == Chord::key(Key::Num3) => Some(2),
                    c if c == Chord::key(Key::Enter) => u8::try_from(choice.selected).ok(),
                    _ => None,
                };
                if let Some(index) = index
                    && usize::from(index) < n
                {
                    let (meas, name) = (choice.meas, choice.name.clone());
                    self.overlay = Overlay::None;
                    self.call(
                        out,
                        Command::DelayInsert {
                            meas,
                            pick: DelayPick::Ranked { index },
                        },
                        format!("{name}: candidate {} inserted", index + 1),
                    );
                    return;
                }
                // The list stays up over the plots and the other keys keep working, except
                // the stimulus's.
                self.overlay_fallthrough(chord, keymap, out);
                return;
            }
            Overlay::PaneMenu(menu) => {
                let mut menu = *menu;
                let n = self.pane_candidates(menu.pane).len();
                match chord.key {
                    Key::ArrowDown | Key::ArrowUp if n > 0 => {
                        let d = if chord.key == Key::ArrowDown {
                            1
                        } else {
                            n - 1
                        };
                        menu.index = (menu.index + d) % n;
                        self.overlay = Overlay::PaneMenu(menu);
                    }
                    Key::Home | Key::PageUp => {
                        menu.index = 0;
                        self.overlay = Overlay::PaneMenu(menu);
                    }
                    Key::End | Key::PageDown => {
                        menu.index = n.saturating_sub(1);
                        self.overlay = Overlay::PaneMenu(menu);
                    }
                    Key::Enter => {
                        self.overlay = Overlay::None;
                        let id = self
                            .pane_candidates(menu.pane)
                            .get(menu.index)
                            .map(|m| m.id);
                        if let Some(id) = id {
                            self.pane_show(menu.pane, id);
                        }
                    }
                    _ => {}
                }
                return;
            }
            Overlay::None => {}
        }
        if let Some(c) = keymap.lookup(self.scope(), chord) {
            self.run_key_command(c, chord, keymap, out);
        }
    }

    /// A key a window that stays out of the way (help, the delay candidates) does not use
    /// runs its command, unless it is the stimulus's: with a window open the stimulus keys
    /// never act.
    fn overlay_fallthrough(&mut self, chord: Chord, keymap: &Keymap, out: &mut Vec<Request>) {
        let Some(c) = keymap.lookup(self.scope(), chord) else {
            return;
        };
        if RESERVED.iter().any(|(_, id)| *id == c) || c == CommandId::StimulusTakeOver {
            return;
        }
        self.run_key_command(c, chord, keymap, out);
    }

    /// Runs the command of a key; a text window it opens does not also type the key.
    fn run_key_command(
        &mut self,
        c: CommandId,
        chord: Chord,
        keymap: &Keymap,
        out: &mut Vec<Request>,
    ) {
        let before = std::mem::discriminant(&self.overlay);
        self.command(c, keymap, out);
        let opened_text = matches!(
            self.overlay,
            Overlay::Palette(_) | Overlay::Prompt(_) | Overlay::Form(_) | Overlay::Settings(_)
        );
        if opened_text && std::mem::discriminant(&self.overlay) != before {
            self.swallow_text = typed_char(&chord);
        }
    }

    /// Esc with a window open: the topmost window closes (a dialog over a view closes back
    /// to the view) and the stimulus is left alone, except that the sweep dialog leaves
    /// nothing armed behind it. A playing stimulus keeps playing: the stop chord or the
    /// strip's Stop stops it.
    fn close_overlay(&mut self, out: &mut Vec<Request>) {
        match &mut self.overlay {
            Overlay::Settings(s) if s.inner_open() => {
                s.close_inner();
            }
            Overlay::Form(f) if f.kind == FormKind::Sweep => {
                self.overlay = Overlay::None;
                self.disarm_unfired(out);
            }
            _ => self.overlay = Overlay::None,
        }
    }

    /// Disarms a stimulus armed (or arming) that is not playing: nothing is left armed
    /// behind a window that was closed. One that plays is left playing.
    fn disarm_unfired(&mut self, out: &mut Vec<Request>) {
        self.stimulus.arm_after_stop = false;
        let firing = self.daemon().is_some_and(|s| s.generator.firing);
        if matches!(self.stimulus.phase, StimPhase::Armed | StimPhase::Arming) && !firing {
            self.stimulus.phase = StimPhase::Stopping;
            out.push(Request::StimStop);
        }
    }

    fn text(&mut self, t: &str) {
        let mut t = t.to_string();
        if let Some(c) = self.swallow_text.take() {
            let mut chars = t.chars();
            if chars
                .next()
                .is_some_and(|first| first.to_ascii_lowercase() == c)
            {
                t = chars.collect();
            }
        }
        match &mut self.overlay {
            Overlay::Palette(p) => p.type_text(&t),
            Overlay::Form(f) => f.type_text(&t),
            Overlay::Settings(_) => self.settings_text(&t),
            Overlay::Prompt(p) => {
                p.text.push_str(&t);
                p.error = None;
            }
            _ => {}
        }
    }

    fn prompt(&mut self, kind: PromptKind, text: String) {
        self.overlay = Overlay::Prompt(Prompt {
            kind,
            text,
            error: None,
        });
    }

    fn apply_prompt(&mut self, out: &mut Vec<Request>) {
        let Overlay::Prompt(p) = &self.overlay else {
            return;
        };
        let kind = p.kind;
        let text = p.text.clone();
        let r = match kind {
            PromptKind::StimulusLevel => self.set_level_text(&text, out),
            PromptKind::CalDelete => self.cal_delete(&text, out),
            PromptKind::TraceMicCurve(id) => {
                let mics = self.daemon().map(|s| s.mics.clone()).unwrap_or_default();
                trace_mic_request(&mics, id, &text).map(|r| out.push(r))
            }
            PromptKind::MicCurveInput => parse_input_curve(&text).and_then(|(channel, curve)| {
                let mut row = self.input_setup(channel);
                let Some(mic) = row.mic.clone() else {
                    return Err(format!(
                        "input {} has no mic name: name it first (palette: Input setup…)",
                        u32::from(channel) + 1
                    ));
                };
                if let CurveChoice::Curve { label } = &curve
                    && self
                        .daemon()
                        .is_none_or(|s| ac2_proto::cal::curve(&s.mics, &mic, label).is_none())
                {
                    let stored = self
                        .daemon()
                        .and_then(|s| ac2_proto::cal::mic(&s.mics, &mic))
                        .map_or_else(|| "none".to_owned(), |m| ac2_scene::cal::labels(&m.curves));
                    return Err(format!("{mic} has no curve {label:?} (stored: {stored})"));
                }
                row.curve = curve;
                let what = curve_what(&row);
                out.push(Request::Call {
                    what,
                    cmd: Command::SessionInputs { inputs: vec![row] },
                });
                Ok(())
            }),
            PromptKind::FinderBand => parse_band(&text).map(|(lo, hi)| {
                self.set_finder(FinderChoice {
                    band: FinderBand::Custom {
                        lo_hz: Hz(lo),
                        hi_hz: Hz(hi),
                    },
                    observation: None,
                });
            }),
            PromptKind::FinderObservation => self.set_observation(&text),
            PromptKind::Offset(id) => {
                parse_number(&text, &["db"]).map(|v| self.set_live_offset(id, v))
            }
            PromptKind::TraceOffset(id) => parse_number(&text, &["db"]).and_then(|v| {
                let t = self.trace_meta(id)?;
                let mut edit = t.edit.clone();
                edit.offset = Db(v);
                let what = format!("{}: offset {}", trace_label(&t), format::db_readout(v));
                out.push(Request::Call {
                    cmd: Command::TraceUpdate { trace: id, edit },
                    what,
                });
                Ok(())
            }),
            PromptKind::TraceRename(id) => {
                let name = text.trim();
                if name.is_empty() {
                    Err("type a name".to_string())
                } else {
                    self.trace_meta(id).map(|t| {
                        let mut edit = t.edit.clone();
                        edit.name = name.to_string();
                        let what = format!("{} renamed to {name}", t.edit.name);
                        out.push(Request::Call {
                            cmd: Command::TraceUpdate { trace: id, edit },
                            what,
                        });
                    })
                }
            }
            PromptKind::TraceExport(id) => {
                let path = text.trim();
                if path.is_empty() {
                    Err("type a file or folder path".to_string())
                } else {
                    self.trace_meta(id).map(|t| {
                        let path = std::path::PathBuf::from(path);
                        let path = match &self.export_dir {
                            Some(d) if path.is_relative() => d.join(path),
                            _ => path,
                        };
                        // The next export starts in the same folder.
                        self.export_dir = if path.is_dir() {
                            Some(path.clone())
                        } else {
                            path.parent()
                                .filter(|p| !p.as_os_str().is_empty())
                                .map(std::path::Path::to_path_buf)
                        };
                        out.push(Request::ExportTrace {
                            trace: id,
                            name: t.edit.name.clone(),
                            path,
                        });
                    })
                }
            }
            PromptKind::TraceSlot(id) => parse_slot(&text).and_then(|slot| {
                let t = self.trace_meta(id)?;
                let mut edit = t.edit.clone();
                edit.slot = slot;
                let what = match slot {
                    Some(n) => format!("{} in slot {n}", t.edit.name),
                    None => format!("{}: slot freed", t.edit.name),
                };
                out.push(Request::Call {
                    cmd: Command::TraceUpdate { trace: id, edit },
                    what,
                });
                Ok(())
            }),
            PromptKind::ImportFile(role) => {
                let path = text.trim();
                if path.is_empty() {
                    Err("type a file path".to_string())
                } else {
                    out.push(Request::Import {
                        path: std::path::PathBuf::from(path),
                        role,
                    });
                    Ok(())
                }
            }
            PromptKind::SessionSave | PromptKind::SessionLoad => {
                parse_session_ref(&text).map(|session| {
                    let load = kind == PromptKind::SessionLoad;
                    let what = match &session {
                        SessionRef::Name { name } => name.clone(),
                        SessionRef::Path { path } => path.clone(),
                    };
                    let (cmd, what) = if load {
                        (
                            Command::FileLoad { session },
                            format!("session {what:?} loaded (disarmed)"),
                        )
                    } else {
                        (
                            Command::FileSave { session },
                            format!("session {what:?} saved"),
                        )
                    };
                    out.push(Request::Call { cmd, what });
                })
            }
            PromptKind::ReplayRecording => parse_session_ref(&text).map(|r| {
                let (recording, what) = match r {
                    SessionRef::Name { name } => (
                        ac2_proto::model::RecordingRef::Name { name: name.clone() },
                        name,
                    ),
                    SessionRef::Path { path } => (
                        ac2_proto::model::RecordingRef::Path { path: path.clone() },
                        path,
                    ),
                };
                out.push(Request::Call {
                    cmd: Command::SessionReplay {
                        recording,
                        pace: ac2_proto::model::ReplayPace::Realtime,
                    },
                    what: format!("replaying {what:?}"),
                });
            }),
            PromptKind::InputMics => parse_mics(&text).map(|mics| {
                let inputs: Vec<InputSetup> = mics
                    .into_iter()
                    .map(|(channel, mic)| {
                        let mut row = self.input_setup(channel);
                        if row.mic != mic {
                            row.mic = mic;
                            // Another capsule: its own curves, chosen again.
                            row.curve = CurveChoice::NotChosen;
                        }
                        row
                    })
                    .collect();
                out.push(Request::Call {
                    what: format!("input setup ({})", mics_text(&inputs)),
                    cmd: Command::SessionInputs { inputs },
                });
            }),
            PromptKind::Delay(id) => parse_number(&text, &["ms"]).and_then(|v| {
                if !(0.0..=10_000.0).contains(&v) {
                    return Err("delay must be 0 … 10000 ms".to_string());
                }
                out.push(Request::Call {
                    cmd: Command::DelaySet {
                        meas: id,
                        delay: Seconds(v / 1000.0),
                    },
                    what: format!("delay {}", format::delay(v / 1000.0)),
                });
                Ok(())
            }),
        };
        match r {
            Ok(()) => self.overlay = Overlay::None,
            Err(e) => {
                if let Overlay::Prompt(p) = &mut self.overlay {
                    p.error = Some(e);
                }
            }
        }
    }

    fn set_level_text(&mut self, text: &str, out: &mut Vec<Request>) -> Result<(), String> {
        let v = parse_number(text, &["dbfs", "db"])?;
        if v > 0.0 {
            return Err("level must be ≤ 0 dBFS".into());
        }
        if let Some(c) = self.ceiling()
            && v > c.0
        {
            return Err(format!("above the daemon's ceiling {}", dbfs(c.0)));
        }
        self.stimulus.level = Some(Dbfs(v));
        if self.stimulus.phase == StimPhase::Idle {
            self.toast(format!("level {} · Space arms", dbfs(v)));
        }
        self.resend_stimulus(out);
        Ok(())
    }

    /// Sends the current settings when armed or firing (level / output change).
    fn resend_stimulus(&mut self, out: &mut Vec<Request>) {
        let firing = match self.stimulus.phase {
            StimPhase::Armed => false,
            StimPhase::Firing | StimPhase::FireRequested => true,
            _ => return,
        };
        if let Some(settings) = self.stimulus.settings() {
            out.push(Request::StimSet(GeneratorDesired {
                settings,
                armed: true,
                firing,
            }));
        }
    }

    fn step_level(&mut self, db: f64, out: &mut Vec<Request>) {
        let Some(l) = self.stimulus.level else {
            self.error("no stimulus level yet: type one (L)");
            return;
        };
        let ceiling = self.ceiling().map_or(0.0, |c| c.0);
        let v = (l.0 + db).clamp(LEVEL_FLOOR, ceiling.min(0.0));
        // Steps land on whole decibels so ↑ then ↓ returns exactly.
        let v = (v * 10.0).round() / 10.0;
        if v == l.0 {
            if db > 0.0 {
                self.toast(format!("at the ceiling {}", dbfs(ceiling)));
            }
            return;
        }
        self.stimulus.level = Some(Dbfs(v));
        self.resend_stimulus(out);
    }

    /// The sweep view (the sweep pane focused, maximised or not): there the stimulus keys
    /// re-sweep; on every other view they drive the generator for live measuring.
    pub fn sweep_view(&self) -> bool {
        self.layout.focus == PaneKind::Distortion
    }

    /// What the stimulus keys start next, for the top bar: armed, what Enter fires; idle,
    /// what Space arms on the focused view. `None` while a request is in flight or playing.
    pub fn stimulus_next(&self) -> Option<(NextKey, NextStimulus)> {
        let level = self.stimulus.level;
        match self.stimulus.phase {
            StimPhase::Armed => Some((
                NextKey::Enter,
                match &self.sweep.plan {
                    Some(p) => NextStimulus::Sweep {
                        again: self.sweep.again,
                        duration_s: p.request.sweep.duration.0,
                        level,
                    },
                    None => self.generator_stimulus(),
                },
            )),
            StimPhase::Idle if self.sweep_view() => Some((
                NextKey::Space,
                self.sweep
                    .last
                    .as_ref()
                    .map_or(NextStimulus::SweepDialog, |p| NextStimulus::Sweep {
                        again: true,
                        duration_s: p.request.sweep.duration.0,
                        level: p.request.level,
                    }),
            )),
            StimPhase::Idle => Some((NextKey::Space, self.generator_stimulus())),
            _ => None,
        }
    }

    /// The generator as the live views play it: never a sweep signal.
    fn generator_stimulus(&self) -> NextStimulus {
        NextStimulus::Generator {
            signal: match self.stimulus.signal {
                Signal::Ess { .. } => Signal::Pink,
                s => s,
            },
            level: self.stimulus.level,
            outputs: self.stimulus.outputs.clone(),
            labels: self.daemon().map(|s| s.outputs.clone()).unwrap_or_default(),
        }
    }

    /// Space: arms what the focused view plays (the sweep view a re-sweep, the others the
    /// generator). Armed with the other kind and still silent, Space re-sets the armed
    /// stimulus to this view's, so what Enter fires is always what a Space on this view
    /// chose; a playing stimulus is never changed by it.
    fn space(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if self.sweep_view() {
            self.space_sweep(force, keymap, out);
        } else {
            self.space_generator(force, keymap, out);
        }
    }

    fn space_sweep(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        // Already set up with a sweep (arming, armed, playing, or queued behind a stop).
        if self.sweep.run.is_some()
            || (self.sweep.plan.is_some()
                && (self.stimulus.phase != StimPhase::Idle || self.sweep.arm_after_stop))
        {
            return;
        }
        let Some(last) = self.sweep.last.clone() else {
            self.open_sweep_dialog(keymap, out);
            return;
        };
        if !self.connected() {
            self.error("not connected");
            return;
        }
        if self.daemon().is_some_and(|s| s.session.open.is_none()) {
            self.error(format!(
                "no audio session to sweep: {}",
                open_session_hint(keymap)
            ));
            return;
        }
        // The level was typed for that sweep; the ceiling may have come down since.
        if let (Some(l), Some(c)) = (last.request.level, self.ceiling())
            && l.0 > c.0
        {
            self.error(format!(
                "the last sweep's {} is above the daemon's ceiling {}: Shift+S sets a new one",
                dbfs(l.0),
                dbfs(c.0)
            ));
            return;
        }
        let plan = SweepPlan {
            name: self.resweep_name(&last.name),
            request: last.request,
        };
        let phase = self.stimulus.phase;
        // A noise queued behind the stop gives way to the sweep this view arms.
        self.stimulus.arm_after_stop = false;
        self.sweep.again = true;
        self.arm_sweep(plan, force, out);
        match phase {
            StimPhase::Stopping => {
                self.toast("stopping… arms the re-sweep once the stop is done (Esc cancels)");
            }
            StimPhase::Armed => self.toast(format!(
                "armed: re-{} · Enter plays the sweep · Esc stops",
                self.stimulus.describe()
            )),
            _ => {}
        }
    }

    fn space_generator(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if self.sweep.run.is_some() {
            return;
        }
        if self.sweep.plan.is_some() {
            match self.stimulus.phase {
                StimPhase::Armed => {
                    self.end_sweep_mode();
                    self.resend_stimulus(out);
                    self.toast(format!(
                        "armed: {} · Enter fires · Esc stops",
                        self.stimulus.describe()
                    ));
                    return;
                }
                StimPhase::Stopping | StimPhase::Idle => {
                    self.sweep.arm_after_stop = false;
                    self.end_sweep_mode();
                }
                _ => return,
            }
        }
        self.arm(force, keymap, out);
    }

    /// `Sweep 1` → `Sweep 2`: the last name's stem with the first number no stored trace
    /// has taken.
    fn resweep_name(&self, last: &str) -> String {
        let stem = match last.rsplit_once(' ') {
            Some((s, n)) if !s.is_empty() && n.parse::<u32>().is_ok() => s,
            _ => last,
        };
        let taken: Vec<&str> = self
            .daemon()
            .map(|s| s.traces.iter().map(|t| t.edit.name.as_str()).collect())
            .unwrap_or_default();
        (2u32..)
            .map(|n| format!("{stem} {n}"))
            .find(|n| !taken.contains(&n.as_str()))
            .unwrap_or_else(|| last.to_string())
    }

    fn arm(&mut self, force: bool, keymap: &Keymap, out: &mut Vec<Request>) {
        if !self.connected() {
            self.error("not connected");
            return;
        }
        if self.stimulus.level.is_none() {
            self.prompt(PromptKind::StimulusLevel, String::new());
            self.toast("type a level first; nothing is armed without one");
            return;
        }
        if self.daemon().is_some_and(|s| s.session.open.is_none()) {
            self.error(format!(
                "no audio session to play into: {}",
                open_session_hint(keymap)
            ));
            return;
        }
        if self.stimulus.phase == StimPhase::Stopping && !force {
            if !self.stimulus.arm_after_stop {
                self.stimulus.arm_after_stop = true;
                self.toast("stopping… arms once the stop is done (Esc cancels)");
            }
            return;
        }
        if !matches!(self.stimulus.phase, StimPhase::Idle) && !force {
            return;
        }
        if let Some(settings) = self.stimulus.settings() {
            self.stimulus.phase = StimPhase::Arming;
            self.armed_with = Some(settings.clone());
            out.push(Request::StimArm { settings, force });
        }
    }

    /// The arm Space asked for while a stop was in flight, now that the stop has landed:
    /// the same checks as a fresh Space, against the state the stop left.
    fn arm_after_stopped(&mut self, out: &mut Vec<Request>) {
        if !self.connected() || self.stimulus.phase != StimPhase::Idle {
            return;
        }
        if self.daemon().is_some_and(|s| s.session.open.is_none()) {
            self.error("not armed: no audio session to play into");
            return;
        }
        if self.daemon().is_some_and(|s| s.session.stopped.is_some()) {
            self.error("not armed: the audio stopped; the daemon is reopening the session");
            return;
        }
        if let Some(settings) = self.stimulus.settings() {
            self.stimulus.phase = StimPhase::Arming;
            self.armed_with = Some(settings.clone());
            out.push(Request::StimArm {
                settings,
                force: false,
            });
        }
    }

    /// The sweep dialog over the open session's inputs and outputs, by name.
    fn open_sweep_dialog(&mut self, keymap: &Keymap, out: &mut Vec<Request>) {
        let Some(o) = self.open_session().cloned() else {
            self.error(format!(
                "no audio session to sweep: {}",
                open_session_hint(keymap)
            ));
            return;
        };
        let inputs = self.session_input_names();
        let outputs = self.session_output_names();
        let mics: Vec<u16> = self
            .daemon()
            .map(|s| {
                s.inputs
                    .iter()
                    .filter(|i| i.mic.is_some())
                    .map(|i| i.channel)
                    .collect()
            })
            .unwrap_or_default();
        let sweeps = self.daemon().map_or(0, |s| {
            s.traces
                .iter()
                .filter(|t| t.kind == TraceKind::Sweep)
                .count()
        });
        let f = Form::sweep(
            Some(&o),
            sweeps,
            &inputs,
            &outputs,
            &mics,
            self.stimulus.level,
        );
        self.overlay = Overlay::Form(Box::new(f));
        if self.devices.is_none() {
            out.push(Request::Devices);
        }
    }

    /// The dialog's sweep becomes the stimulus: armed with it (or re-sent when already
    /// armed); Enter then plays it.
    fn arm_sweep(&mut self, plan: SweepPlan, force: bool, out: &mut Vec<Request>) {
        let r = &plan.request;
        self.stimulus.signal = Signal::Ess { sweep: r.sweep };
        self.stimulus.level = r.level;
        self.stimulus.outputs = r.outputs.clone();
        self.sweep.plan = Some(plan);
        match self.stimulus.phase {
            StimPhase::Idle => {
                if let Some(settings) = self.stimulus.settings() {
                    self.stimulus.phase = StimPhase::Arming;
                    self.armed_with = Some(settings.clone());
                    out.push(Request::StimArm { settings, force });
                }
            }
            StimPhase::Firing | StimPhase::FireRequested => {
                self.error(format!(
                    "the stimulus is firing: {STOP_ANYWHERE} stops it, then arm the sweep"
                ));
                self.sweep.plan = None;
            }
            // The stop before it is still on its way (stop, then the lease given back): the
            // sweep arms once it has landed, not into the lease that stop is releasing.
            StimPhase::Stopping => self.sweep.arm_after_stop = true,
            _ => self.resend_stimulus(out),
        }
    }

    /// Enter while armed with a sweep: `ir.capture` with the current level and outputs.
    fn fire_sweep(&mut self, out: &mut Vec<Request>) {
        let Some(plan) = &self.sweep.plan else {
            return;
        };
        let mut request = plan.request.clone();
        request.level = self.stimulus.level;
        request.outputs = self.stimulus.outputs.clone();
        if let Signal::Ess { sweep } = self.stimulus.signal {
            request.sweep = sweep;
        }
        let name = plan.name.clone();
        self.stimulus.phase = StimPhase::FireRequested;
        self.sweep.last = Some(SweepPlan {
            request: request.clone(),
            name: name.clone(),
        });
        out.push(Request::Sweep { request, name });
    }

    /// A stop ends the sweep set-up: the next arm is noise again.
    fn end_sweep_mode(&mut self) {
        if self.sweep.plan.take().is_some() {
            self.stimulus.signal = Signal::Pink;
        }
    }

    /// Acts on the run this client started as the mirror reports it: stored → the pane
    /// shows it; failed → says why. The daemon disarms once the sweep has played, so the
    /// stimulus is off from then on; when the run ends the lease is given back and sweep
    /// mode ends — the next sweep is armed from the dialog again.
    fn follow_sweep(&mut self, out: &mut Vec<Request>) {
        let Some(id) = self.sweep.run else {
            return;
        };
        let Some(r) = self.daemon().and_then(|s| s.sweep.clone()) else {
            return;
        };
        if r.id != id || self.sweep.seen.as_ref() == Some(&r.status) {
            return;
        }
        self.sweep.seen = Some(r.status.clone());
        if !matches!(r.status, SweepStatus::Playing { .. })
            && matches!(
                self.stimulus.phase,
                StimPhase::Firing | StimPhase::FireRequested
            )
        {
            self.stimulus.phase = StimPhase::Idle;
        }
        match r.status {
            SweepStatus::Playing { .. } => {}
            SweepStatus::Analysing => {
                self.toast("sweep recorded: analysing");
            }
            SweepStatus::Done { trace } => {
                self.sweep.run = None;
                // The new result is selected: listed highlighted, named in the captions,
                // and what the trace keys change.
                self.selected_trace = Some(trace);
                self.sweep.shown = Some(trace);
                self.sweep.fit = Some(trace);
                self.fit_new_sweep();
                self.release_after_sweep(out);
                self.layout.shown[PaneKind::Distortion.index()] = true;
                self.layout.focus = PaneKind::Distortion;
                self.toast(format!(
                    "sweep stored as {:?}: U dB / %, H impulse response, Shift+S sweeps again",
                    r.name
                ));
            }
            SweepStatus::Failed { msg, .. } => {
                self.sweep.run = None;
                self.release_after_sweep(out);
                self.error(format!("sweep failed: {msg}"));
            }
        }
    }

    /// A finished run ends sweep mode and gives the lease back, unless the operator has
    /// armed again meanwhile. Only a lease this client holds: a stop without one is the
    /// universal stop and would silence whoever took the stimulus over.
    fn release_after_sweep(&mut self, out: &mut Vec<Request>) {
        if self.stimulus.phase != StimPhase::Idle {
            return;
        }
        self.end_sweep_mode();
        let mine = self
            .daemon()
            .and_then(|s| s.generator.owner.as_ref())
            .is_some_and(|o| Some(o) == self.my_client_id());
        if mine {
            self.stimulus.phase = StimPhase::Stopping;
            self.sweep.releasing = true;
            out.push(Request::StimStop);
        }
    }

    /// Sweep traces with their data, oldest first.
    pub fn sweep_traces(&self) -> Vec<(&TraceData, &GridDef)> {
        self.traces
            .values()
            .filter(|(t, _)| t.meta.kind == TraceKind::Sweep && t.sweep.is_some())
            .map(|(t, g)| (t.as_ref(), g.as_ref()))
            .collect()
    }

    /// The sweep trace the distortion pane shows: the selected trace when it is a sweep,
    /// else the sweep selected last, else the newest.
    pub fn shown_sweep(&self) -> Option<(&TraceData, &GridDef)> {
        let all = self.sweep_traces();
        let find = |id: TraceId| all.iter().find(|(t, _)| t.meta.id == id).copied();
        self.selected_trace
            .and_then(find)
            .or_else(|| self.sweep.shown.and_then(find))
            .or_else(|| all.last().copied())
    }

    /// N / Shift+N on the sweep pane: the next / previous stored sweep, selected (the
    /// transfer pane and the trace keys follow it).
    fn cycle_sweep(&mut self, d: i32) {
        let ids: Vec<TraceId> = self.sweep_traces().iter().map(|(t, _)| t.meta.id).collect();
        if ids.is_empty() {
            self.error("no sweep results yet: Shift+S sets one up");
            return;
        }
        let cur = self.shown_sweep().map(|(t, _)| t.meta.id);
        let i = cur
            .and_then(|c| ids.iter().position(|x| *x == c))
            .unwrap_or(0) as i32;
        let id = ids[(i + d).rem_euclid(ids.len() as i32) as usize];
        self.select_trace(Some(id));
        if let Some((t, _)) = self.shown_sweep() {
            let name = trace_label(&t.meta);
            self.toast(format!("{name} selected"));
        }
    }

    /// The one trace selection: a sweep selected is also what the sweep pane shows.
    fn select_trace(&mut self, id: Option<TraceId>) {
        self.selected_trace = id;
        if let Some(id) = id
            && self.daemon().is_some_and(|s| {
                s.traces
                    .iter()
                    .any(|t| t.id == id && t.kind == TraceKind::Sweep)
            })
        {
            self.sweep.shown = Some(id);
        }
    }

    fn set_finder(&mut self, f: FinderChoice) {
        self.finder = f;
        self.toast(format!("delay finder: {} · X finds", f.describe()));
    }

    fn set_observation(&mut self, text: &str) -> Result<(), String> {
        let observation = if text.trim().is_empty() {
            None
        } else {
            let v = parse_number(text, &["s"])?;
            if !(v > 0.0 && v <= MAX_OBSERVATION_S) {
                return Err(format!(
                    "observation must be above 0 and at most {MAX_OBSERVATION_S} s"
                ));
            }
            Some(Seconds(v))
        };
        let f = FinderChoice {
            observation,
            ..self.finder
        };
        if let Some(o) = observation
            && f.sub_like()
            && !SUB_OBSERVATIONS_S.contains(&o.0)
        {
            return Err("the sub band observes 2, 4 or 8 s".into());
        }
        self.set_finder(f);
        Ok(())
    }

    /// The prompt text of a calibration to delete: the selected measurement's input and its
    /// mic name.
    fn cal_delete_text(&self) -> String {
        self.selected_meas()
            .and_then(|m| meas_input(&m.config.kind))
            .map(|input| {
                let mic = self.input_setup(input).mic.unwrap_or_default();
                format!("{}={mic}", u32::from(input) + 1)
            })
            .unwrap_or_default()
    }

    fn cal_delete(&mut self, text: &str, out: &mut Vec<Request>) -> Result<(), String> {
        let (channel, mic) = match parse_mics(text)?.as_slice() {
            [(c, Some(m))] => (*c, m.clone()),
            [(c, None)] => {
                return Err(format!("type the mic name after {}=", u32::from(*c) + 1));
            }
            _ => return Err("one input=mic".into()),
        };
        let Some(device) = self
            .daemon()
            .and_then(|s| s.session.open.as_ref())
            .map(|o| o.input_device.clone())
        else {
            return Err(
                "no open session: calibrations of other devices are deleted with `ac2 cal rm --device`"
                    .into(),
            );
        };
        out.push(Request::Call {
            what: format!(
                "sensitivity calibration of {mic} on input {} deleted",
                u32::from(channel) + 1
            ),
            cmd: Command::CalDelete {
                key: CalKey {
                    device,
                    channel,
                    mic,
                },
            },
        });
        Ok(())
    }

    /// The stimulus outputs follow the open session's output device: the ones last used on
    /// it, output 1 on a device never used (decision K4). Outputs never change under a held
    /// stimulus; a device change re-opens the session, which disarms it anyway.
    fn follow_output_device(&mut self) {
        let dev = self
            .daemon()
            .and_then(|s| s.session.open.as_ref())
            .map(|o| o.output_device.0.clone());
        let Some(dev) = dev else {
            return;
        };
        if self.stim_device.as_ref() == Some(&dev) || self.stimulus.phase != StimPhase::Idle {
            return;
        }
        self.stimulus.outputs = self
            .prefs
            .outputs_for(&dev)
            .map_or_else(|| vec![0], <[u16]>::to_vec);
        self.stim_device = Some(dev);
    }

    fn need_meas(&mut self, want: &[fn(&MeasKind) -> bool], what: &str) -> Option<Measurement> {
        match self.selected_meas() {
            Some(m) if want.iter().any(|f| f(&m.config.kind)) => Some(m.clone()),
            _ => {
                self.error(format!("select a {what} measurement first (N)"));
                None
            }
        }
    }

    /// The daemon's input setup row of `channel` (default: no mic name, no curve chosen).
    fn input_setup(&self, channel: u16) -> InputSetup {
        ac2_proto::cal::input_setup(
            self.daemon()
                .map(|s| s.inputs.as_slice())
                .unwrap_or_default(),
            channel,
        )
    }

    /// Moves the selected transfer measurement's delay by `samples` (`delay.nudge`); the
    /// daemon keeps the averages where it can, so the curve moves at once.
    fn nudge_delay(&mut self, samples: f64, out: &mut Vec<Request>) {
        let Some(m) = self.need_tf() else {
            return;
        };
        let Some(rate) = self.open_session().map(|s| f64::from(s.sample_rate_hz)) else {
            self.error(format!(
                "{}: no audio session (a delay in samples needs its rate)",
                m.config.name
            ));
            return;
        };
        self.call(
            out,
            Command::DelayNudge {
                meas: m.id,
                by: Seconds(samples / rate),
            },
            format!("{}: delay {}", m.config.name, format::sample_step(samples)),
        );
    }

    fn need_tf(&mut self) -> Option<Measurement> {
        self.need_meas(
            &[|k| matches!(k, MeasKind::Transfer { .. })],
            "transfer-function",
        )
    }

    /// Focuses `p` and selects the measurement it shows.
    /// Focuses pane `p` from the keyboard: it selects the measurement the pane shows,
    /// unless the selected stored trace is drawn there (the sweep chosen on the sweep pane
    /// stays selected on the way to the transfer pane).
    fn focus(&mut self, p: PaneKind) {
        self.layout.shown[p.index()] = true;
        self.layout.focus = p;
        let keep = self
            .selected_trace_meta()
            .filter(|t| drawn_in(t, p))
            .map(|t| t.id);
        self.select_shown(p);
        if keep.is_some() {
            self.selected_trace = keep;
        }
    }

    /// Selects the measurement pane `p` shows, if any.
    fn select_shown(&mut self, p: PaneKind) {
        if let Some(id) = self.pane_meas(p).map(|m| m.id) {
            self.select(id);
        }
    }

    /// Selects `id` (deselecting a slot) and makes it what its pane shows.
    fn select(&mut self, id: MeasId) {
        self.selected = Some(id);
        self.selected_trace = None;
        if let Some(p) = self.meas(id).map(|m| PaneKind::for_kind(&m.config.kind)) {
            self.pane_meas.insert(p, id);
        }
    }

    /// Pane `p` shows `id`; it gets the focus and `id` is selected.
    fn pane_show(&mut self, p: PaneKind, id: MeasId) {
        if matches!(self.overlay, Overlay::PaneMenu(_)) {
            self.overlay = Overlay::None;
        }
        let Some(m) = self.meas(id) else {
            return;
        };
        if !p.shows(&m.config.kind) {
            return;
        }
        self.layout.shown[p.index()] = true;
        self.layout.focus = p;
        self.select(id);
    }

    /// The measurement list of pane `p`, the shown one highlighted.
    fn pane_menu(&mut self, p: PaneKind) -> Overlay {
        let c = self.pane_candidates(p);
        if c.is_empty() {
            self.error(format!("no {} measurements", p.what()));
            return Overlay::None;
        }
        let shown = self.pane_meas(p).map(|m| m.id);
        let index = c.iter().position(|m| Some(m.id) == shown).unwrap_or(0);
        Overlay::PaneMenu(PaneMenu { pane: p, index })
    }

    /// Sets the smoothing of what the smoothing keys act on (`step`: +1 coarser / -1 finer
    /// through [`SMOOTHING_STEPS`]; `to`: an explicit setting instead).
    fn smooth(&mut self, step: i32, to: Option<Option<SmoothingFraction>>, out: &mut Vec<Request>) {
        let Some(target) = self.smooth_target() else {
            self.error("no transfer or spectrum measurement to smooth");
            return;
        };
        let label = target.label();
        match target.kind() {
            Smoothable::Transfer | Smoothable::Spectrum => {}
            Smoothable::Rta => {
                self.toast(format!(
                    "{label}: RTA bands already are fractional-octave; smoothing applies to \
                     spectra and transfer functions"
                ));
                return;
            }
            Smoothable::No => {
                self.error(format!(
                    "{label}: smoothing applies to transfer and spectrum curves only"
                ));
                return;
            }
        }
        let cur = target.smoothing();
        let want = match to {
            Some(f) => f,
            None => {
                let i = SMOOTHING_STEPS
                    .iter()
                    .position(|f| *f == cur.map(|s| s.fraction))
                    .unwrap_or(0) as i32;
                let j = (i + step).clamp(0, SMOOTHING_STEPS.len() as i32 - 1);
                if i == j {
                    self.toast(format!(
                        "{label}: {} is the {}",
                        target.caption(),
                        if step > 0 { "widest" } else { "finest" }
                    ));
                    return;
                }
                SMOOTHING_STEPS[j as usize]
            }
        };
        let new = want.map(|fraction| match target.kind() {
            Smoothable::Spectrum => spectrum_smoothing(fraction),
            // Phase is smoothed with the magnitude unless the curve was set to keep it.
            _ => Smoothing {
                fraction,
                mode: cur.map_or(SmoothingMode::MagnitudePhase, |s| s.mode),
            },
        });
        match target {
            SmoothTarget::Trace(t) => {
                if t.edit.locked {
                    self.error(format!("{label} is locked"));
                    return;
                }
                let mut t = t.clone();
                t.edit.smoothing = new;
                let what = format!("{label}: {}", SmoothTarget::Trace(t.clone()).caption());
                let (trace, edit) = (t.id, t.edit);
                self.call(out, Command::TraceUpdate { trace, edit }, what);
            }
            SmoothTarget::Meas(mut m) => {
                match &mut m.config.kind {
                    MeasKind::Transfer { config } => config.smoothing = new,
                    MeasKind::Spectrum { config } => config.smoothing = want,
                    MeasKind::Math { config } => config.smoothing = new,
                    _ => {}
                }
                let what = format!("{label}: {}", SmoothTarget::Meas(m.clone()).caption());
                let (meas, config) = (m.id, m.config);
                self.call(out, Command::MeasUpdate { meas, config }, what);
            }
        }
    }

    fn cycle_pane(&mut self, d: i32) {
        let vis: Vec<PaneKind> = PaneKind::ALL
            .into_iter()
            .filter(|p| self.layout.is_shown(*p))
            .collect();
        if vis.is_empty() {
            return;
        }
        let i = vis
            .iter()
            .position(|p| *p == self.layout.focus)
            .unwrap_or(0) as i32;
        let n = vis.len() as i32;
        self.focus(vis[((i + d).rem_euclid(n)) as usize]);
    }

    /// N / Shift+N: the next / previous measurement the focused pane can show.
    fn cycle_meas(&mut self, d: i32) {
        let p = self.layout.focus;
        let ids: Vec<MeasId> = self.pane_candidates(p).iter().map(|m| m.id).collect();
        if ids.is_empty() {
            self.error(format!("no {} measurements", p.what()));
            return;
        }
        let i = self
            .pane_meas(p)
            .and_then(|s| ids.iter().position(|x| *x == s.id))
            .map_or(if d > 0 { -1 } else { 0 }, |i| i as i32);
        self.select(ids[((i + d).rem_euclid(ids.len() as i32)) as usize]);
    }

    /// V / Shift+V: the next / previous shown stored trace in the list's order (slotted
    /// or not), with the live measurement as the stop between the last and the first;
    /// `hidden` (Alt) steps through the hidden ones too.
    fn cycle_trace(&mut self, d: i32, hidden: bool) {
        let ids: Vec<TraceId> = self
            .trace_list()
            .into_iter()
            .filter(|t| hidden || t.edit.visible)
            .map(|t| t.id)
            .collect();
        if ids.is_empty() {
            self.error(if hidden {
                "no stored traces (Ctrl+1 … 9 capture one)"
            } else {
                "no shown stored traces (1 … 9 show a slot; Alt+V reaches hidden traces)"
            });
            return;
        }
        // Position 0 is the live measurement, the k-th trace is k + 1. A selected trace off
        // the cycle (hidden) steps from its place in the whole list.
        let n = ids.len() as i32 + 1;
        let i = self.selected_trace.and_then(|s| {
            if let Some(i) = ids.iter().position(|x| *x == s) {
                return Some(i as i32 + 1);
            }
            let all: Vec<TraceId> = self.trace_list().iter().map(|t| t.id).collect();
            let at = all.iter().position(|x| *x == s)?;
            // The traces of the cycle before it: stepping forward lands on the next one.
            let before = all[..at].iter().filter(|x| ids.contains(x)).count() as i32;
            Some(if d > 0 { before } else { before + 1 })
        });
        match (i.unwrap_or(0) + d).rem_euclid(n) {
            0 => self.select_live(),
            k => {
                self.select_trace(Some(ids[(k - 1) as usize]));
                self.reveal_trace();
                if let Some(t) = self.selected_trace_meta() {
                    let hidden = if t.edit.visible { "" } else { " (hidden)" };
                    let label = trace_label(t);
                    self.toast(format!("{label}{hidden} selected"));
                }
            }
        }
    }

    /// Shows or hides stored trace `id`.
    fn toggle_shown(&mut self, id: TraceId, out: &mut Vec<Request>) {
        let Some(t) = self
            .daemon()
            .and_then(|s| s.traces.iter().find(|t| t.id == id))
        else {
            return;
        };
        let mut edit = t.edit.clone();
        edit.visible = !edit.visible;
        let what = format!(
            "{} {}",
            trace_label(t),
            if edit.visible { "shown" } else { "hidden" }
        );
        self.call(out, Command::TraceUpdate { trace: id, edit }, what);
    }

    /// The selected stored trace when its curve is on the transfer pane, where the trace
    /// keys (U, J, `,` `.`, E) act on it; `Ok(None)`: they act on the live measurement. A
    /// locked trace is refused here, saying so.
    fn transfer_trace_for_edit(&mut self) -> Result<Option<TraceMeta>, ()> {
        let Some(t) = self.selected_trace_meta().cloned() else {
            return Ok(None);
        };
        if !matches!(
            t.kind,
            TraceKind::Transfer | TraceKind::Target | TraceKind::Sweep
        ) {
            return Ok(None);
        }
        if t.edit.locked {
            self.error(format!("{} is locked", trace_label(&t)));
            return Err(());
        }
        Ok(Some(t))
    }

    fn select_live(&mut self) {
        self.select_trace(None);
        self.toast("keys act on the live measurement");
    }

    fn call(&mut self, out: &mut Vec<Request>, cmd: Command, what: String) {
        out.push(Request::Call { cmd, what });
    }

    fn command(&mut self, c: CommandId, keymap: &Keymap, out: &mut Vec<Request>) {
        use CommandId as C;
        match c {
            C::Help => {
                self.overlay = if self.overlay == Overlay::Help {
                    Overlay::None
                } else {
                    self.help_scroll = 0.0;
                    Overlay::Help
                };
            }
            C::Palette => {
                self.overlay = if matches!(self.overlay, Overlay::Palette(_)) {
                    Overlay::None
                } else {
                    Overlay::Palette(Palette::default())
                };
            }
            C::Quit => self.quit = true,
            C::Fullscreen => self.fullscreen = !self.fullscreen,
            C::KeyHints => {
                self.prefs.key_hints = !self.prefs.key_hints;
                self.prefs_dirty = true;
                // Off, the line is gone: say how it comes back.
                if !self.prefs.key_hints {
                    let key = keymap
                        .first_chord(CommandId::KeyHints, Scope::Global)
                        .map_or_else(|| "the palette".to_owned(), |c| c.label());
                    self.toast(format!("key hints off · {key} shows them again"));
                }
            }

            C::StimulusArm => self.space(false, keymap, out),
            C::StimulusTakeOver => self.space(true, keymap, out),
            C::StimulusFire => match self.stimulus.phase {
                StimPhase::Armed if self.sweep.plan.is_some() => self.fire_sweep(out),
                StimPhase::Armed => {
                    if let Some(settings) = self.stimulus.settings() {
                        self.stimulus.phase = StimPhase::FireRequested;
                        out.push(Request::StimSet(GeneratorDesired {
                            settings,
                            armed: true,
                            firing: true,
                        }));
                    }
                }
                StimPhase::Idle => self.error("not armed: Space arms first"),
                _ => {}
            },
            C::StimulusStop | C::StopAnywhere => {
                self.stimulus.arm_after_stop = false;
                self.sweep.arm_after_stop = false;
                if self.stimulus_live() {
                    self.stimulus.phase = StimPhase::Stopping;
                    out.push(Request::StimStop);
                }
            }
            C::LevelUp => self.step_level(1.0, out),
            C::LevelDown => self.step_level(-1.0, out),
            C::LevelUpCoarse => self.step_level(3.0, out),
            C::LevelDownCoarse => self.step_level(-3.0, out),
            C::StimulusLevel => {
                let text = self
                    .stimulus
                    .level
                    .map(|l| format::fixed(l.0, 1).replace(format::MINUS, "-"))
                    .unwrap_or_default();
                self.prompt(PromptKind::StimulusLevel, text);
            }
            C::StimulusOutputs => {
                self.open_settings(Page::Io, out);
                // On the stimulus output: the first ticked one, else the first output.
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.focus_outputs = !s.focus_stimulus_outputs();
                }
            }
            C::Settings => {
                let page = self.settings_page_last;
                self.open_settings(page, out);
            }

            C::FocusTransfer => self.focus(PaneKind::Transfer),
            C::FocusSpectrum => self.focus(PaneKind::Spectrum),
            C::FocusIr => self.focus(PaneKind::Ir),
            C::FocusSpl => self.focus(PaneKind::Spl),
            C::FocusDistortion => self.focus(PaneKind::Distortion),
            C::SweepNew => self.open_sweep_dialog(keymap, out),
            C::DistortionUnit => {
                self.view.distortion.unit = match self.view.distortion.unit {
                    DistortionUnit::Db => DistortionUnit::Percent,
                    DistortionUnit::Percent => DistortionUnit::Db,
                };
            }
            C::SweepView => {
                self.view.distortion.mode = self.view.distortion.mode.next();
                self.focus(PaneKind::Distortion);
            }
            C::SweepIr => {
                self.view.distortion.mode = match self.view.distortion.mode {
                    SweepMode::Ir => SweepMode::Response,
                    _ => SweepMode::Ir,
                };
            }
            C::HideDistortion => {
                self.layout.shown[PaneKind::Distortion.index()] = false;
                if self.layout.focus == PaneKind::Distortion {
                    self.layout.focus = PaneKind::Transfer;
                }
            }
            C::NextPane => self.cycle_pane(1),
            C::PrevPane => self.cycle_pane(-1),
            C::MaximizePane => self.cycle_layout(),
            C::NextMeasurement if self.layout.focus == PaneKind::Distortion => {
                self.cycle_sweep(1);
            }
            C::PrevMeasurement if self.layout.focus == PaneKind::Distortion => {
                self.cycle_sweep(-1);
            }
            C::NextMeasurement => self.cycle_meas(1),
            C::PrevMeasurement => self.cycle_meas(-1),
            C::PaneMeasurement => self.overlay = self.pane_menu(self.layout.focus),
            C::NextTrace => self.cycle_trace(1, false),
            C::PrevTrace => self.cycle_trace(-1, false),
            C::NextAnyTrace => self.cycle_trace(1, true),
            C::PrevAnyTrace => self.cycle_trace(-1, true),
            C::ToggleTrace => match self.selected_trace {
                Some(id) => self.toggle_shown(id, out),
                None => self.error(SELECT_TRACE_FIRST),
            },
            C::TraceSlot => match self.selected_trace_meta().cloned() {
                Some(t) => {
                    let text = t.edit.slot.map(|n| n.to_string()).unwrap_or_default();
                    self.prompt(PromptKind::TraceSlot(t.id), text);
                }
                None => self.error(SELECT_TRACE_FIRST),
            },
            C::TraceExport => match self.selected_trace_meta().map(|t| t.id) {
                Some(id) => {
                    // The folder of the last export (else the home directory), ready for a
                    // file name.
                    let text = self
                        .export_dir
                        .as_ref()
                        .map(|d| {
                            let mut s = d.display().to_string();
                            if !s.ends_with(std::path::MAIN_SEPARATOR) {
                                s.push(std::path::MAIN_SEPARATOR);
                            }
                            s
                        })
                        .unwrap_or_default();
                    self.prompt(PromptKind::TraceExport(id), text);
                }
                None => self.error(SELECT_TRACE_FIRST),
            },
            C::TraceRename => match self.selected_trace_meta().cloned() {
                Some(t) => self.prompt(PromptKind::TraceRename(t.id), t.edit.name.clone()),
                None => self.error(SELECT_TRACE_FIRST),
            },
            C::SelectLive => self.select_live(),
            C::DeleteTrace => self.ask_delete_trace(),
            C::SmoothCoarser => self.smooth(1, None, out),
            C::SmoothFiner => self.smooth(-1, None, out),
            C::SmoothOff => self.smooth(0, Some(None), out),
            C::Smooth48 => self.smooth(0, Some(Some(SmoothingFraction::FortyEighth)), out),
            C::Smooth24 => self.smooth(0, Some(Some(SmoothingFraction::TwentyFourth)), out),
            C::Smooth12 => self.smooth(0, Some(Some(SmoothingFraction::Twelfth)), out),
            C::Smooth6 => self.smooth(0, Some(Some(SmoothingFraction::Sixth)), out),
            C::Smooth3 => self.smooth(0, Some(Some(SmoothingFraction::Third)), out),
            C::CycleTheme => {
                self.theme = match self.theme {
                    ThemeName::Dark => ThemeName::Light,
                    ThemeName::Light => ThemeName::HighContrast,
                    ThemeName::HighContrast => ThemeName::Dark,
                };
                self.prefs.theme = Some(self.theme);
                self.prefs_dirty = true;
            }
            C::ZoomIn | C::ZoomOut => {
                let t = self.nav.target;
                let about = self
                    .view
                    .cursor_hz
                    .filter(|h| *h > t.lo && *h < t.hi)
                    .unwrap_or((t.lo * t.hi).sqrt());
                let f = if c == C::ZoomIn {
                    ZOOM_FACTOR
                } else {
                    1.0 / ZOOM_FACTOR
                };
                self.nav.set_target(t.zoom(about, f));
            }
            C::PanLeft | C::PanRight => {
                let d = if c == C::PanLeft {
                    -PAN_OCTAVES
                } else {
                    PAN_OCTAVES
                };
                let t = self.nav.target.pan(d);
                self.nav.set_target(t);
            }
            C::ResetView => self.nav.set_target(FreqRange::default()),
            C::LevelZoomIn
            | C::LevelZoomOut
            | C::LevelPanUp
            | C::LevelPanDown
            | C::LevelFit
            | C::LevelReset => {
                // Fit and reset frame the whole picture: frequency back to 20 Hz – 20 kHz
                // too, as Home alone does.
                if matches!(c, C::LevelFit | C::LevelReset) {
                    self.nav.set_target(FreqRange::default());
                }
                self.level_key(c)
            }
            C::ToggleCursor => {
                let t = self.nav.target;
                self.view.cursor_hz = match self.view.cursor_hz {
                    Some(_) => None,
                    None => Some((t.lo * t.hi).sqrt()),
                };
                // Off takes the spectrograph's time with it; on starts on frequency alone.
                self.view.spectrum.spectrograph.cursor_s = None;
            }
            C::CursorLeft | C::CursorRight => {
                let t = self.nav.target;
                let k = if c == C::CursorLeft { -1.0 } else { 1.0 };
                let hz = self.view.cursor_hz.unwrap_or((t.lo * t.hi).sqrt());
                let hz = (hz * 2f64.powf(k / 12.0)).clamp(t.lo, t.hi);
                self.view.cursor_hz = Some(hz);
            }
            C::Slot1
            | C::Slot2
            | C::Slot3
            | C::Slot4
            | C::Slot5
            | C::Slot6
            | C::Slot7
            | C::Slot8
            | C::Slot9 => {
                let slot = slot_of(c);
                if let Some(m) = self.need_meas(
                    &[MeasKind::publishes_tf, MeasKind::publishes_levels],
                    "transfer, spectrum, RTA or math",
                ) {
                    // The slot's previous trace is replaced unless it is locked; a locked
                    // one only gives up the slot.
                    let replace = self.slots()[usize::from(slot - 1)]
                        .filter(|t| !t.edit.locked)
                        .map(|t| t.id);
                    out.push(Request::Capture {
                        meas: m.id,
                        slot,
                        name: format!("{} S{slot}", m.config.name),
                        replace,
                    });
                }
            }
            C::ShowSlot1
            | C::ShowSlot2
            | C::ShowSlot3
            | C::ShowSlot4
            | C::ShowSlot5
            | C::ShowSlot6
            | C::ShowSlot7
            | C::ShowSlot8
            | C::ShowSlot9 => {
                let slot = slot_of(c);
                match self.slots()[usize::from(slot - 1)].map(|t| t.id) {
                    None => self.error(format!(
                        "slot {slot} is empty (Ctrl+{slot} captures into it)"
                    )),
                    Some(id) => self.toggle_shown(id, out),
                }
            }
            C::ImportTrace => self.prompt(PromptKind::ImportFile(ImportRole::Trace), String::new()),
            C::SessionSave => self.prompt(PromptKind::SessionSave, String::new()),
            C::SessionLoad => self.prompt(PromptKind::SessionLoad, String::new()),
            C::Reconnect => out.push(Request::Reconnect),
            C::OpenSession => {
                if !self.connected() {
                    self.error("not connected");
                } else {
                    self.open_settings(Page::Audio, out);
                }
            }
            C::Record => {
                let recording = self
                    .daemon()
                    .and_then(|s| s.recording.as_ref())
                    .is_some_and(ac2_proto::model::RecordingRun::active);
                match self.open_session().cloned() {
                    _ if recording => {
                        self.call(out, Command::RecStop, "recording stopped".into());
                    }
                    None => self.error(format!(
                        "no audio session to record: {}",
                        open_session_hint(keymap)
                    )),
                    Some(o) => self.call(
                        out,
                        Command::RecStart {
                            request: ac2_proto::model::RecordRequest {
                                inputs: o.config.input_channels.clone(),
                                name: None,
                                max_duration: Seconds(
                                    self.prefs
                                        .record_limit_min
                                        .map_or(RECORD_MAX_S, |m| f64::from(m) * 60.0),
                                ),
                                max_bytes: None,
                            },
                        },
                        "recording started".into(),
                    ),
                }
            }
            C::ReplayRecording => {
                if self.connected() {
                    self.prompt(PromptKind::ReplayRecording, String::new());
                } else {
                    self.error("not connected");
                }
            }
            C::CloseSession => {
                if self.open_session().is_none() {
                    self.error("no audio session is open");
                } else {
                    self.call(out, Command::SessionClose, "audio session closed".into());
                }
            }
            C::NewTransfer | C::NewSpectrum | C::NewRta | C::NewSpl => {
                let kind = match c {
                    C::NewSpectrum => FormKind::Spectrum,
                    C::NewRta => FormKind::Rta,
                    C::NewSpl => FormKind::Spl,
                    _ => FormKind::Transfer,
                };
                match self.open_session().cloned() {
                    None => self.error(format!(
                        "no audio session to measure: {}",
                        open_session_hint(keymap)
                    )),
                    Some(o) => {
                        let names = self.session_input_names();
                        let mics: Vec<u16> = self
                            .daemon()
                            .map(|s| {
                                s.inputs
                                    .iter()
                                    .filter(|i| i.mic.is_some())
                                    .map(|i| i.channel)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let f =
                            Form::measurement(kind, Some(&o), &self.measurements(), &names, &mics);
                        self.overlay = Overlay::Form(Box::new(f));
                        if self.devices.is_none() {
                            // Channel names arrive with the device list.
                            out.push(Request::Devices);
                        }
                    }
                }
            }
            C::NewMath => {
                // A starts as what the focused pane shows: its measurement, or the selected
                // trace.
                let first = self
                    .selected_trace_meta()
                    .map(|t| Operand::Trace { trace: t.id })
                    .or_else(|| {
                        let p = match self.layout.focus {
                            PaneKind::Spectrum => PaneKind::Spectrum,
                            _ => PaneKind::Transfer,
                        };
                        self.pane_meas(p).map(|m| Operand::Meas { meas: m.id })
                    });
                match Form::math(self.math_candidates(), first) {
                    Ok(f) => self.overlay = Overlay::Form(Box::new(f)),
                    Err(e) => self.error(e),
                }
            }
            C::EditMath => {
                let m = self
                    .selected_meas()
                    .filter(|m| matches!(m.config.kind, MeasKind::Math { .. }))
                    .or_else(|| {
                        [PaneKind::Transfer, PaneKind::Spectrum]
                            .into_iter()
                            .filter_map(|p| self.pane_meas(p))
                            .find(|m| matches!(m.config.kind, MeasKind::Math { .. }))
                    })
                    .cloned();
                match m.map(|m| Form::edit_math(&m, self.math_candidates())) {
                    Some(Ok(f)) => self.overlay = Overlay::Form(Box::new(f)),
                    Some(Err(e)) => self.error(e),
                    None => self.error("select a math channel first (N)"),
                }
            }
            C::DeleteMeasurement => match self.selected_meas().cloned() {
                Some(m) => self.call(
                    out,
                    Command::MeasDelete { meas: m.id },
                    format!("{} deleted", m.config.name),
                ),
                None => self.error("select a measurement first (N)"),
            },
            C::InputMics => {
                let rows: Vec<InputSetup> = self
                    .daemon()
                    .map(|s| {
                        s.inputs
                            .iter()
                            .filter(|i| i.mic.is_some())
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                self.prompt(PromptKind::InputMics, mics_text(&rows));
            }
            C::TraceMicCurve => match self.selected_trace_meta().cloned() {
                Some(t) => {
                    // Start from what the trace knows: the applied curve, else the mic it
                    // was captured with.
                    let text = t
                        .mic_curve
                        .as_ref()
                        .map(|m| ac2_scene::cal::curve_name(&m.mic, &m.curve.label))
                        .or_else(|| t.mic.as_ref().map(|m| format!("{} ", m.name)))
                        .unwrap_or_default();
                    self.prompt(PromptKind::TraceMicCurve(t.id), text);
                }
                None => self.error("select a stored trace first (V, or click it in the list)"),
            },
            C::CalDelete => {
                let text = self.cal_delete_text();
                self.prompt(PromptKind::CalDelete, text);
            }
            C::InputSetup | C::Calibrations => match self.daemon() {
                Some(_) => {
                    let page = if c == C::InputSetup {
                        Page::Io
                    } else {
                        Page::Calibration
                    };
                    self.open_settings(page, out);
                }
                None => self.error("not connected to a daemon"),
            },
            C::MicCurveInput => {
                let text = self
                    .selected_meas()
                    .and_then(|m| meas_input(&m.config.kind))
                    .map(|i| format!("{}=", u32::from(i) + 1))
                    .unwrap_or_default();
                self.prompt(PromptKind::MicCurveInput, text);
            }
            C::FinderAuto | C::FinderFull | C::FinderMid | C::FinderSub => {
                let band = match c {
                    C::FinderFull => FinderBand::Full,
                    C::FinderMid => FinderBand::Mid,
                    C::FinderSub => FinderBand::Sub,
                    _ => FinderBand::Auto,
                };
                self.set_finder(FinderChoice {
                    band,
                    observation: None,
                });
            }
            C::FinderCustom => {
                let text = match self.finder.band {
                    FinderBand::Custom { lo_hz, hi_hz } => format!("{}-{}", lo_hz.0, hi_hz.0),
                    _ => String::new(),
                };
                self.prompt(PromptKind::FinderBand, text);
            }
            C::FinderObservation => {
                let text = self
                    .finder
                    .observation
                    .map(|o| o.0.to_string())
                    .unwrap_or_default();
                self.prompt(PromptKind::FinderObservation, text);
            }
            C::MicCurve => {
                if let Some(m) = self.need_meas(
                    &[|k| {
                        matches!(
                            k,
                            MeasKind::Transfer { .. }
                                | MeasKind::Spectrum { .. }
                                | MeasKind::Rta { .. }
                                | MeasKind::Spl { .. }
                        )
                    }],
                    "transfer, spectrum, RTA or SPL",
                ) && let Some(input) = meas_input(&m.config.kind)
                {
                    let mut row = self.input_setup(input);
                    let Some(mic) = row.mic.clone() else {
                        self.error(format!(
                            "input {} has no mic name: name it first (palette: Input setup…)",
                            u32::from(input) + 1
                        ));
                        return;
                    };
                    let mics = self.daemon().map(|s| s.mics.clone()).unwrap_or_default();
                    ac2_proto::cal::settle(&mut row, &mics);
                    row.curve =
                        ac2_proto::cal::step(&row.curve, ac2_proto::cal::mic(&mics, &mic), true);
                    let what = curve_what(&row);
                    self.call(out, Command::SessionInputs { inputs: vec![row] }, what);
                }
            }

            C::Freeze => {
                if let Some(m) = self.need_meas(
                    &[MeasKind::publishes_tf, MeasKind::publishes_levels],
                    "transfer or spectrum",
                ) {
                    let frozen = !m.frozen;
                    let what = if frozen { "frozen" } else { "unfrozen" };
                    self.call(
                        out,
                        Command::MeasFreeze { meas: m.id, frozen },
                        format!("{} {what}", m.config.name),
                    );
                }
            }
            C::ResetAverage => {
                let m = self.focused_pane_meas();
                if let Some(m) = &m
                    && let MeasKind::Math { config } = &m.config.kind
                {
                    // A math channel holds no averaging of its own: starting it over means
                    // starting its live operands over.
                    for o in config.expr.operands() {
                        if let Operand::Meas { meas } = o {
                            self.call(
                                out,
                                Command::MeasReset { meas },
                                format!("{}: operands' averaging reset", m.config.name),
                            );
                        }
                    }
                } else if let Some(m) = m {
                    self.call(
                        out,
                        Command::MeasReset { meas: m.id },
                        format!("{} averaging reset", m.config.name),
                    );
                }
            }
            C::StartStop => {
                if let Some(m) = self.focused_pane_meas() {
                    let (cmd, what) = if m.running {
                        (Command::MeasStop { meas: m.id }, "stopped")
                    } else {
                        (Command::MeasStart { meas: m.id }, "started")
                    };
                    self.call(out, cmd, format!("{} {what}", m.config.name));
                }
            }

            C::InsertDelay | C::InsertStrongest => {
                if let Some(m) = self.need_tf() {
                    let pick = if c == C::InsertDelay {
                        DelayPick::FirstArrival
                    } else {
                        DelayPick::Strongest
                    };
                    if matches!(self.overlay, Overlay::DelayPick(_)) {
                        self.overlay = Overlay::None;
                    }
                    out.push(Request::FindDelay {
                        meas: m.id,
                        pick,
                        band: self.finder.band,
                        observation: self.finder.observation,
                    });
                }
            }
            C::TypeDelay => {
                if let Some(m) = self.need_tf() {
                    let text = m
                        .delay
                        .as_ref()
                        .map(|d| format::delay_entry_ms(d.applied.0))
                        .unwrap_or_default();
                    self.prompt(PromptKind::Delay(m.id), text);
                }
            }
            C::DelayDown | C::DelayUp | C::DelayDownFine | C::DelayUpFine => {
                let step = match c {
                    C::DelayDown => -1.0,
                    C::DelayUp => 1.0,
                    C::DelayDownFine => -DELAY_FINE_STEP,
                    _ => DELAY_FINE_STEP,
                };
                self.nudge_delay(step, out);
            }
            C::TrackDelay => {
                if let Some(m) = self.need_tf() {
                    let enabled = !m.delay.as_ref().is_some_and(|d| d.tracking);
                    let what = if enabled { "on" } else { "off" };
                    self.call(
                        out,
                        Command::DelayTrack {
                            meas: m.id,
                            enabled,
                        },
                        format!("{}: tracking {what}", m.config.name),
                    );
                }
            }
            C::Invert => match self.transfer_trace_for_edit() {
                Err(()) => {}
                Ok(Some(t)) if t.kind == TraceKind::Target => {
                    self.error(format!("{}: a target curve has no phase", trace_label(&t)));
                }
                Ok(Some(t)) => {
                    let mut edit = t.edit.clone();
                    edit.polarity = match edit.polarity {
                        Polarity::Normal => Polarity::Inverted,
                        Polarity::Inverted => Polarity::Normal,
                    };
                    let what = format!(
                        "{}: polarity {}",
                        trace_label(&t),
                        match edit.polarity {
                            Polarity::Normal => "normal",
                            Polarity::Inverted => "inverted",
                        }
                    );
                    self.call(out, Command::TraceUpdate { trace: t.id, edit }, what);
                }
                Ok(None) => {
                    if let Some(m) = self.need_tf() {
                        let e = self.edits.entry(m.id).or_default();
                        e.inverted = !e.inverted;
                    }
                }
            },
            C::Offset => self.offset_prompt(),
            C::OffsetUp => self.step_offset(Some(1.0), out),
            C::OffsetDown => self.step_offset(Some(-1.0), out),
            C::OffsetUpCoarse => self.step_offset(Some(3.0), out),
            C::OffsetDownCoarse => self.step_offset(Some(-3.0), out),
            C::OffsetClear => self.step_offset(None, out),
            C::NudgeEarlier | C::NudgeLater => {
                let d = if c == C::NudgeEarlier {
                    -NUDGE_S
                } else {
                    NUDGE_S
                };
                // Whole steps: repeated nudges never accumulate float error.
                let step = |v: f64| ((v + d) / NUDGE_S).round() * NUDGE_S;
                match self.transfer_trace_for_edit() {
                    Err(()) => {}
                    Ok(Some(t)) if t.kind == TraceKind::Target => {
                        self.error(format!("{}: a target curve has no phase", trace_label(&t)));
                    }
                    Ok(Some(t)) => {
                        let mut edit = t.edit.clone();
                        edit.delay_nudge = Seconds(step(edit.delay_nudge.0));
                        let what = format!(
                            "{}: nudged {}",
                            trace_label(&t),
                            format::ms(edit.delay_nudge.0, 1)
                        );
                        self.call(out, Command::TraceUpdate { trace: t.id, edit }, what);
                    }
                    Ok(None) => {
                        if let Some(m) = self.need_tf() {
                            let e = self.edits.entry(m.id).or_default();
                            e.nudge_s = step(e.nudge_s);
                        }
                    }
                }
            }
            C::PhaseReference => match self.transfer_trace_for_edit() {
                Err(()) => {}
                Ok(Some(t)) if t.kind == TraceKind::Target => {
                    self.error(format!("{}: a target curve has no phase", trace_label(&t)));
                }
                Ok(Some(t)) => {
                    self.view.tf.phase_reference = Some(TraceKey::Stored(t.id));
                    self.toast(format!("phase reference: {}", trace_label(&t)));
                }
                Ok(None) => {
                    if let Some(m) = self.need_tf() {
                        self.view.tf.phase_reference = Some(TraceKey::Live(m.id));
                        self.toast(format!("phase reference: {}", m.config.name));
                    }
                }
            },
            C::Target => self.prompt(PromptKind::ImportFile(ImportRole::Target), String::new()),
            C::Average => self.average(AverageMethod::Power, out),
            C::AverageComplex => self.average(AverageMethod::Complex, out),
            C::AverageCoherence => self.average(AverageMethod::CoherenceWeighted, out),
            C::ToggleIr => {
                let i = PaneKind::Ir.index();
                self.layout.shown[i] = !self.layout.shown[i];
                if !self.layout.shown[i] && self.layout.focus == PaneKind::Ir {
                    self.layout.focus = PaneKind::Transfer;
                }
            }
            C::CoherenceMask => {
                let cur = self.view.tf.coherence.blank_below;
                let i = COHERENCE_MASKS.iter().position(|m| *m == cur).unwrap_or(0);
                let next = COHERENCE_MASKS[(i + 1) % COHERENCE_MASKS.len()];
                self.view.tf.coherence.blank_below = next;
                self.toast(match next {
                    None => "coherence mask off".to_string(),
                    Some(t) => format!("coherence mask: hide γ² < {t:.1}"),
                });
            }
            C::CoherencePlacement => {
                self.view.tf.coherence_placement = match self.view.tf.coherence_placement {
                    CoherencePlacement::Pane => CoherencePlacement::OverlayOnMagnitude,
                    CoherencePlacement::OverlayOnMagnitude => CoherencePlacement::Pane,
                };
            }
            C::PhaseUnwrap => {
                self.view.tf.phase = match self.view.tf.phase {
                    PhaseView::Wrapped => PhaseView::Unwrapped {
                        range: Range::new(-720.0, 180.0),
                    },
                    _ => PhaseView::Wrapped,
                };
            }
            C::GroupDelay => {
                self.view.tf.phase = match self.view.tf.phase {
                    PhaseView::GroupDelay { .. } => PhaseView::Wrapped,
                    _ => PhaseView::GroupDelay {
                        range_ms: Range::new(-2.0, 10.0),
                    },
                };
            }
            C::SpectrumStyle => {
                self.view.spectrum.style = match self.view.spectrum.style {
                    SpectrumStyle::Bars => SpectrumStyle::Line,
                    SpectrumStyle::Line => SpectrumStyle::Bars,
                };
            }
            C::PeakHold => {
                self.view.spectrum.peak_hold = !self.view.spectrum.peak_hold;
                self.peaks.clear();
            }
            C::Spectrograph => {
                let before = self.view.spectrum.mode;
                let mode = before.next();
                self.view.spectrum.mode = mode;
                // The history starts with the view and goes with it: nothing is kept for a
                // hidden spectrograph; from the split to the spectrograph alone it stays.
                if !mode.spectrograph() {
                    self.spectrographs.clear();
                    self.view.spectrum.spectrograph.cursor_s = None;
                } else if !before.spectrograph()
                    && let Some(d) = self.data.clone()
                {
                    self.fold_spectrographs(&d);
                }
                self.focus(PaneKind::Spectrum);
            }
            C::SpectrographSpan => {
                let sg = &mut self.view.spectrum.spectrograph;
                sg.span_s = ac2_scene::view::SpectrographView::next_span(sg.span_s);
                sg.cursor_s = sg.cursor_s.filter(|t| *t <= f64::from(sg.span_s));
                let span = sg.span_s;
                // Slots of another length cannot hold the frames already placed.
                self.spectrographs.clear();
                self.toast(format!("spectrograph: last {span} s"));
            }
            C::IrMode => {
                self.view.ir.mode = match self.view.ir.mode {
                    IrMode::Linear => IrMode::Log,
                    IrMode::Log => IrMode::Etc,
                    IrMode::Etc => IrMode::Linear,
                };
            }
            C::SplLeqView | C::SplShowMeter | C::SplShowLeq | C::SplShowMeterLeq => {
                self.view.spl.mode = match c {
                    C::SplShowMeter => SplMode::Meter,
                    C::SplShowLeq => SplMode::Leq,
                    C::SplShowMeterLeq => SplMode::MeterLeq,
                    _ => self.view.spl.mode.next(),
                };
                self.focus(PaneKind::Spl);
            }
            // Either shows the windows (a layout change is about them) and is remembered.
            C::SplLeqStyle | C::SplLeqHistory => {
                let l = &mut self.view.spl.layout;
                if c == C::SplLeqStyle {
                    l.style = match l.style {
                        LeqStyle::Columns => LeqStyle::Tiles,
                        LeqStyle::Tiles => LeqStyle::Columns,
                    };
                } else {
                    l.history = !l.history;
                }
                self.view.spl.mode = self.view.spl.mode.with_leq();
                self.focus(PaneKind::Spl);
                self.prefs.leq = self.view.spl.layout;
                self.prefs_dirty = true;
            }
            C::SplNewLog => match self.pane_meas(PaneKind::Spl).cloned() {
                Some(m) => {
                    let MeasKind::Spl { config } = &m.config.kind else {
                        return;
                    };
                    let zone = self.local_zone;
                    let run = self
                        .leq_run(m.id)
                        .map(|r| ac2_scene::leq::run_text(&r, &config.leq, |t| zone.offset_s(t)));
                    self.overlay = Overlay::NewLog(Box::new(NewLogPrompt {
                        meas: m.id,
                        meter: m.config.name.clone(),
                        confirm: ac2_scene::leq::new_log_confirm(
                            &m.config.name,
                            &config.leq,
                            run.as_ref(),
                        ),
                    }));
                }
                None => self.error("no SPL meter: make one first (New SPL meter… in Ctrl+K)"),
            },
            C::SplTimeWeighting
            | C::SplWeighting
            | C::SplFast
            | C::SplSlow
            | C::SplImpulse
            | C::SplA
            | C::SplC
            | C::SplZ => self.spl_weightings(c, out),
            C::LeqWindows => match self.pane_meas(PaneKind::Spl) {
                Some(_) => self.open_settings(Page::Leq, out),
                None => self.error("no SPL meter: make one first (New SPL meter… in Ctrl+K)"),
            },
        }
    }

    /// F / Z in the SPL pane (or the palette's direct choices): the meter the pane shows
    /// takes the next (or the named) time or frequency weighting, in place — its Leq
    /// windows, log and interval carry on. The pane keeps showing what it showed.
    fn spl_weightings(&mut self, c: CommandId, out: &mut Vec<Request>) {
        use CommandId as C;
        use ac2_proto::model::{TimeWeighting as T, Weighting as W};
        let Some(mut m) = self.pane_meas(PaneKind::Spl).cloned() else {
            self.error("no SPL meter: make one first (New SPL meter… in Ctrl+K)");
            return;
        };
        let MeasKind::Spl { config } = &mut m.config.kind else {
            return;
        };
        match c {
            C::SplTimeWeighting => {
                config.time_weighting = match config.time_weighting {
                    T::Fast => T::Slow,
                    T::Slow => T::Impulse,
                    T::Impulse => T::Fast,
                }
            }
            C::SplWeighting => {
                config.weighting = match config.weighting {
                    W::A => W::C,
                    W::C => W::Z,
                    W::Z => W::A,
                }
            }
            C::SplFast => config.time_weighting = T::Fast,
            C::SplSlow => config.time_weighting = T::Slow,
            C::SplImpulse => config.time_weighting = T::Impulse,
            C::SplA => config.weighting = W::A,
            C::SplC => config.weighting = W::C,
            C::SplZ => config.weighting = W::Z,
            _ => return,
        }
        let what = format!(
            "{}: {}",
            m.config.name,
            ac2_scene::spl::metric_name(config.weighting, config.time_weighting)
        );
        // The pane keeps its view: the Leq windows do not follow the meter's weightings, so
        // switching to the meter would read as the windows having changed. The daemon's
        // reply names the new metric either way.
        self.focus(PaneKind::Spl);
        let (meas, config) = (m.id, m.config);
        self.call(out, Command::MeasUpdate { meas, config }, what);
    }

    /// How long the SPL meter's number holds a reading under time weighting `t`: the
    /// preference when set, else the time weighting's own display period.
    pub fn spl_display_period_s(&self, t: ac2_proto::model::TimeWeighting) -> f64 {
        self.prefs.spl_hold_ms.map_or_else(
            || ac2_scene::spl::display_period_s(t),
            |ms| f64::from(ms) / 1e3,
        )
    }

    /// Takes each SPL meter's newest frame as its displayed reading when the display
    /// period has passed ([`ac2_scene::spl::SplHold`]).
    fn fold_spl(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        if let Some(st) = self.daemon() {
            let ids: Vec<MeasId> = st.measurements.iter().map(|m| m.id).collect();
            self.spl_hold.retain(|k, _| ids.contains(k));
        }
        for tf in d.latest.frames.values() {
            let FrameData::Spl(f) = &tf.frame.data else {
                continue;
            };
            let period = self.spl_display_period_s(f.meta.time_weighting);
            let mut held = self.spl_hold.remove(&f.meas);
            ac2_scene::spl::SplHold::update(
                &mut held,
                f,
                tf.frame.stamp.capture_wall_ns.0,
                tf.frame.stamp.config_rev,
                period,
            );
            if let Some(h) = held {
                self.spl_hold.insert(f.meas, h);
            }
        }
    }

    /// The run of SPL meter `meas`'s log, from its newest `leq` frame.
    fn leq_run(&self, meas: MeasId) -> Option<ac2_proto::frame::LeqRun> {
        use ac2_proto::FrameData;
        self.data
            .as_ref()
            .and_then(|d| {
                d.latest.get(&Topic::Data {
                    meas,
                    stream: Stream::Leq,
                })
            })
            .and_then(|f| match &f.frame.data {
                FrameData::Leq(l) => l.meta.run,
                _ => None,
            })
    }

    /// Whether SPL meter `meas` reads dB SPL (its newest frame says so).
    fn leq_calibrated(&self, meas: MeasId) -> bool {
        use ac2_proto::FrameData;
        use ac2_proto::model::LevelScale;
        let scale = |s: Stream| {
            self.data
                .as_ref()
                .and_then(|d| d.latest.get(&Topic::Data { meas, stream: s }))
                .and_then(|f| match &f.frame.data {
                    FrameData::Leq(l) => Some(l.meta.scale),
                    FrameData::Spl(l) => Some(l.meta.scale),
                    _ => None,
                })
        };
        scale(Stream::Leq).or_else(|| scale(Stream::Spl)) == Some(LevelScale::DbSpl)
    }

    fn leq_msg(&mut self, m: LeqMsg, out: &mut Vec<Request>) {
        if m == LeqMsg::Submit {
            self.submit_leq(out);
            return;
        }
        let Some(d) = self.overlay.leq_mut() else {
            return;
        };
        match m {
            LeqMsg::Focus(f) => {
                d.focus = f;
                d.select_all();
            }
            LeqMsg::Cycle(f, step) => {
                d.focus = f;
                d.cycle(step);
            }
            LeqMsg::Add => d.add_window(),
            LeqMsg::Remove => d.remove_window(),
            LeqMsg::Cancel => self.overlay = Overlay::None,
            LeqMsg::Submit => {}
        }
    }

    /// Enter on the Leq dialog: the meter's windows go out (applied in place: its log and
    /// windows carry on) and the SPL pane shows them; or the dialog says what is wrong.
    fn submit_leq(&mut self, out: &mut Vec<Request>) {
        let Some(d) = self.overlay.leq_mut() else {
            return;
        };
        match d.meas_config() {
            Ok(config) => {
                let (meas, name) = (d.meas, d.name.clone());
                self.overlay = Overlay::None;
                self.view.spl.mode = self.view.spl.mode.with_leq();
                self.focus(PaneKind::Spl);
                self.call(
                    out,
                    Command::MeasUpdate { meas, config },
                    format!("{name}: Leq windows set"),
                );
            }
            Err(e) => d.error = Some(e),
        }
    }

    /// Folds the newest `leq` frame of each SPL meter into its history.
    fn fold_leq(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        // Without a synced daemon state (a resync, a short drop) the meter list is unknown,
        // not empty: keep every history until the state says a meter is gone.
        if self.daemon().is_none() {
            return;
        }
        let cfgs: BTreeMap<MeasId, ac2_proto::model::LeqConfig> = self
            .measurements()
            .iter()
            .filter_map(|m| match &m.config.kind {
                MeasKind::Spl { config } => Some((m.id, config.leq.clone())),
                _ => None,
            })
            .collect();
        self.leq_history.retain(|k, _| cfgs.contains_key(k));
        for tf in d.latest.frames.values() {
            let FrameData::Leq(f) = &tf.frame.data else {
                continue;
            };
            let Some(cfg) = cfgs.get(&f.meas) else {
                continue;
            };
            let seq = tf.frame.stamp.seq;
            let e = self.leq_history.entry(f.meas).or_default();
            if seq > e.0 {
                e.0 = seq;
                e.1.push(cfg, f, tf.frame.stamp.capture_wall_ns.0 as f64 / 1e9);
            }
        }
    }

    /// Rebuilds each SPL meter's history from its log when the app first sees the meter
    /// (connected, resynced, created), when its windows change and when a new log starts
    /// (from this app or any other client: the log empties, or its row count starts over);
    /// a new log first clears the history. The frames received go on folding in meanwhile.
    fn follow_leq_logs(&mut self, data: Option<&DataSnapshot>, out: &mut Vec<Request>) {
        use ac2_proto::FrameData;
        let Some(st) = self.daemon() else {
            // Resyncing: the daemon may have restarted with other logs; ask again once synced.
            self.leq_logs.clear();
            return;
        };
        let mut logged: BTreeMap<MeasId, u64> = BTreeMap::new();
        if let Some(d) = data {
            for tf in d.latest.frames.values() {
                if let FrameData::Leq(f) = &tf.frame.data {
                    logged.insert(f.meas, f.meta.logged);
                }
            }
        }
        let meters: Vec<(MeasId, ac2_proto::model::LeqConfig, Option<bool>)> = st
            .measurements
            .iter()
            .filter_map(|m| match &m.config.kind {
                MeasKind::Spl { config } => Some((
                    m.id,
                    config.leq.clone(),
                    st.spl_logs
                        .iter()
                        .find(|l| l.meas == m.id)
                        .map(|l| l.started_at.is_some()),
                )),
                _ => None,
            })
            .collect();
        self.leq_logs
            .retain(|k, _| meters.iter().any(|(id, ..)| id == k));
        for (meas, config, started) in meters {
            let logged = logged.get(&meas).copied();
            let seen = self.leq_logs.get(&meas);
            let new_log = seen.is_some_and(|s| {
                logged.is_some_and(|l| l < s.logged) || (s.started && started == Some(false))
            });
            if new_log && let Some((_, h)) = self.leq_history.get_mut(&meas) {
                h.clear();
            }
            match self.leq_logs.get_mut(&meas) {
                Some(s) if !new_log && s.config == config => {
                    s.logged = logged.unwrap_or(s.logged).max(s.logged);
                    s.started = started.unwrap_or(s.started);
                }
                _ => {
                    self.leq_backfill_ask += 1;
                    let ask = self.leq_backfill_ask;
                    self.leq_logs.insert(
                        meas,
                        LeqLogSeen {
                            config,
                            logged: logged.unwrap_or(0),
                            started: started.unwrap_or(false),
                            ask,
                        },
                    );
                    out.push(Request::LeqBackfill { meas, ask });
                }
            }
        }
    }

    /// A meter's history rebuilt from its log: under the frames received, unless a newer
    /// rebuild was asked for meanwhile.
    fn leq_backfilled(
        &mut self,
        meas: MeasId,
        ask: u64,
        result: Result<Box<ac2_proto::model::SplHistory>, String>,
    ) {
        if self.leq_logs.get(&meas).is_none_or(|s| s.ask != ask) {
            return;
        }
        match result {
            Ok(h) => self.leq_history.entry(meas).or_default().1.backfill(&h),
            Err(e) => {
                let name = self
                    .measurements()
                    .iter()
                    .find(|m| m.id == meas)
                    .map(|m| m.config.name.clone())
                    .unwrap_or_default();
                self.error(format!("{name}: Leq history from the log: {e}"));
            }
        }
    }

    /// Toasts each window that went over its limit or came back since the last mirror; the
    /// alarms already there when the app connected are history, not news.
    fn follow_leq_alarms(&mut self) {
        let Some(st) = self.daemon() else {
            return;
        };
        let names: BTreeMap<MeasId, String> = st
            .measurements
            .iter()
            .map(|m| (m.id, m.config.name.clone()))
            .collect();
        let mut news = Vec::new();
        let mut seen = BTreeMap::new();
        for l in &st.spl_logs {
            let newest = l.alarms.last().copied();
            match self.leq_alarms_seen.get(&l.meas) {
                None => {}
                Some(prev) => {
                    let from = match prev {
                        None => 0,
                        Some(p) => l.alarms.iter().rposition(|a| a == p).map_or(0, |i| i + 1),
                    };
                    let name = names.get(&l.meas).cloned().unwrap_or_default();
                    news.extend(l.alarms[from..].iter().map(|a| (name.clone(), *a)));
                }
            }
            seen.insert(l.meas, newest);
        }
        self.leq_alarms_seen = seen;
        for (name, a) in news {
            match ac2_scene::leq::alarm_text(&name, &a) {
                (true, text) => self.error(text),
                (false, text) => self.toast(text),
            }
        }
    }

    fn conn_event(&mut self, e: ConnEvent, keymap: &Keymap, out: &mut Vec<Request>) {
        match e {
            ConnEvent::Connecting { target } => {
                if !matches!(self.conn, ConnState::Failed { .. }) {
                    self.conn = ConnState::Connecting { target };
                }
            }
            ConnEvent::Connected {
                target,
                server,
                client_id,
            } => {
                self.conn = ConnState::Connected {
                    target,
                    server,
                    client_id,
                };
                // A new connection holds no lease.
                self.stimulus.phase = StimPhase::Idle;
                self.stimulus.arm_after_stop = false;
                // Nor has it rebuilt any history: the log may have moved on meanwhile.
                self.leq_logs.clear();
            }
            ConnEvent::Failed { target, error, .. } => {
                self.conn = ConnState::Failed { target, error };
                self.pending_select = None;
                self.mirror = None;
                self.data = None;
                self.stimulus.phase = StimPhase::Idle;
                self.stimulus.arm_after_stop = false;
            }
            ConnEvent::Mirror(v) => {
                self.mirror = Some(v);
                // The daemon disarmed the generator when the audio stopped; an arm queued
                // behind a stop must not follow it into a session that is reopening.
                if self.stimulus.arm_after_stop
                    && self.daemon().is_some_and(|s| s.session.stopped.is_some())
                {
                    self.stimulus.arm_after_stop = false;
                    self.error("not armed: the audio stopped; the daemon is reopening the session");
                }
                let ids: Vec<MeasId> = self.measurements().iter().map(|m| m.id).collect();
                if let Some(p) = self.pending_select
                    && ids.contains(&p)
                {
                    self.selected = Some(p);
                    self.pending_select = None;
                }
                let waiting = self.pending_select.is_some() && self.pending_select == self.selected;
                if !waiting && self.selected.is_none_or(|s| !ids.contains(&s)) {
                    self.selected = self
                        .measurements()
                        .iter()
                        .find(|m| matches!(m.config.kind, MeasKind::Transfer { .. }))
                        .or(self.measurements().first())
                        .map(|m| m.id);
                }
                self.edits.retain(|k, _| ids.contains(k));
                self.pane_meas.retain(|_, m| ids.contains(m));
                let metas: BTreeMap<TraceId, TraceMeta> = self
                    .daemon()
                    .map(|st| st.traces.iter().map(|t| (t.id, t.clone())).collect())
                    .unwrap_or_default();
                // Fetched data keeps its columns; its metadata follows the mirror (edits,
                // visibility and slots change by event). The smoothing stays the one the
                // columns were served with until the re-smoothed data arrives.
                self.traces.retain(|k, _| metas.contains_key(k));
                for (id, (data, _)) in &mut self.traces {
                    if let Some(m) = metas.get(id) {
                        follow_meta(data, m);
                    }
                }
                if self.selected_trace.is_some_and(|t| !metas.contains_key(&t)) {
                    self.selected_trace = None;
                }
                if let Some(TraceKey::Stored(r)) = self.view.tf.phase_reference
                    && !metas.contains_key(&r)
                {
                    self.view.tf.phase_reference = None;
                }
                self.follow_spectrum_starts();
                self.follow_output_device();
                self.follow_settings();
                self.follow_sweep(out);
                self.follow_leq_alarms();
                let data = self.data.clone();
                self.follow_leq_logs(data.as_deref(), out);
                if self.open_session_when_empty && self.connected() && self.daemon().is_some() {
                    self.open_session_when_empty = false;
                    if self.open_session().is_none() && self.overlay == Overlay::None {
                        self.command(CommandId::OpenSession, keymap, out);
                    }
                }
            }
            ConnEvent::Devices(r) => {
                if let Ok(d) = &r {
                    self.devices = Some(d.clone());
                }
                let names = self.session_input_names();
                match (&mut self.overlay, r) {
                    (Overlay::Settings(s), Ok(d)) => {
                        s.session.set_backends(d, &self.prefs);
                        if s.focus_outputs {
                            s.focus_outputs = !s.focus_stimulus_outputs();
                        }
                    }
                    (Overlay::Settings(s), Err(e)) => {
                        s.session.error = Some(format!("cannot list devices: {e}"));
                    }
                    (Overlay::Form(f), Ok(_)) => {
                        // Channel names arrived: relabel the inputs.
                        relabel(f, &names);
                    }
                    // The dialog was closed meanwhile.
                    _ => {}
                }
            }
            ConnEvent::Preview {
                backend,
                device,
                result,
            } => {
                // The answer for a device the dialog has since left says nothing about the
                // one it shows now.
                if let Some(s) = self.overlay.session_mut()
                    && s.preview_target() == Some((backend, device))
                {
                    s.preview_error = result.err().map(|e| format!("meters unavailable: {e}"));
                }
            }
            ConnEvent::LoopbackDetected(r) => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.session.detect_result(r);
                    // The detection closed the preview on the daemon: reopen it now.
                    self.preview_sent_s = f64::NEG_INFINITY;
                } else if let Err(e) = r {
                    self.error(format!("detect loopback: {e}"));
                }
            }
            ConnEvent::SessionOpened { transfers } => {
                let none_yet = self.daemon().is_some_and(|s| s.measurements.is_empty());
                if none_yet && !transfers.is_empty() && self.overlay == Overlay::None {
                    self.overlay = Overlay::Offer(Box::new(Offer { transfers }));
                }
            }
            ConnEvent::LeqBackfill { meas, ask, result } => {
                self.leq_backfilled(meas, ask, result);
            }
            ConnEvent::MeasCreated(m) => {
                self.selected = Some(m.id);
                self.pending_select = Some(m.id);
            }
            ConnEvent::Data(d) => {
                if self.view.spectrum.peak_hold {
                    self.fold_peaks(&d);
                }
                if self.view.spectrum.mode.spectrograph() {
                    self.fold_spectrographs(&d);
                }
                // A new log clears the history before its first frame goes in.
                self.follow_leq_logs(Some(&d), out);
                self.fold_leq(&d);
                self.fold_spl(&d);
                self.data = Some(d);
                self.fit_started_spectra();
            }
            ConnEvent::Trace(mut t, g) => {
                if let Some(m) = self
                    .daemon()
                    .and_then(|s| s.traces.iter().find(|x| x.id == t.meta.id))
                    .cloned()
                {
                    follow_meta(&mut t, &m);
                }
                self.traces.insert(t.meta.id, (t, g));
                self.fit_new_sweep();
            }
            ConnEvent::Reply { what, result } => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    s.cal.reply(&what, &result);
                }
                match result {
                    Ok(()) => self.toast(what),
                    Err(e) => self.error(format!("{what}: {e}")),
                }
            }
            ConnEvent::DelayFound {
                meas,
                pick,
                finding,
            } => self.delay_found(meas, pick, *finding, out),
            ConnEvent::Captured { slot, trace } => {
                self.toast(format!("slot {slot}: {} captured", trace.edit.name));
            }
            ConnEvent::Stimulus(s) => self.stim_event(s, out),
            ConnEvent::Server { what, result } => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    match (&result, &what) {
                        // A failed change leaves the info shown as it was.
                        (Err(_), Some(_)) => {}
                        _ => s.connection.server = Some(result.clone()),
                    }
                }
                match (what, result) {
                    (Some(w), Ok(_)) => self.toast(w),
                    (Some(w), Err(e)) => self.error(format!("{w}: {e}")),
                    (None, _) => {}
                }
            }
        }
    }

    fn form_msg(&mut self, m: FormMsg, out: &mut Vec<Request>) {
        match m {
            FormMsg::Submit => self.submit_form(out),
            // As Esc: the sweep dialog leaves nothing armed behind it.
            FormMsg::Cancel => {
                if matches!(self.overlay, Overlay::Form(_)) {
                    self.close_overlay(out);
                }
            }
            FormMsg::Focus(i) => {
                if let Overlay::Form(f) = &mut self.overlay {
                    f.focus_field(i);
                }
            }
            FormMsg::Cycle(i, d) => {
                if let Overlay::Form(f) = &mut self.overlay {
                    f.focus_field(i);
                    f.cycle(d);
                }
            }
        }
    }

    /// Enter on a dialog: the command goes out and the dialog closes, or the dialog says
    /// what is wrong and stays.
    fn submit_form(&mut self, out: &mut Vec<Request>) {
        let open = self.open_session().cloned();
        let ceiling = self.ceiling();
        if let Overlay::Form(f) = &mut self.overlay
            && f.kind == FormKind::Sweep
        {
            match f.sweep_plan(open.as_ref(), ceiling) {
                Ok(plan) => {
                    self.overlay = Overlay::None;
                    self.sweep.again = false;
                    self.arm_sweep(plan, false, out);
                }
                Err(e) => f.error = Some(e),
            }
            return;
        }
        let Overlay::Form(f) = &mut self.overlay else {
            return;
        };
        let edit = f.math_edit();
        let r = f.meas_config(open.as_ref()).map(|config| match edit {
            Some(meas) => Request::Call {
                what: format!("{} changed", config.name),
                cmd: Command::MeasUpdate { meas, config },
            },
            None => Request::CreateMeas { config },
        });
        match r {
            Ok(req) => {
                out.push(req);
                self.overlay = Overlay::None;
            }
            Err(e) => f.error = Some(e),
        }
    }

    /// Keys of the calibrations view.
    fn cal_view_key(&mut self, chord: Chord, swallow: Option<char>, out: &mut Vec<Request>) {
        use eframe::egui::Key;
        let Some(st) = self.mirror.as_ref().and_then(|m| m.state.clone()) else {
            self.overlay = Overlay::None;
            return;
        };
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let v = &mut s.cal;
        if let Some(d) = &mut v.acoustic {
            if let Some(a) = crate::acoustic_dialog::key(d, &chord) {
                self.cal_action(a, out);
            } else {
                self.swallow_text = swallow;
            }
            return;
        }
        if let Some(d) = &mut v.electrical {
            if let Some(a) = crate::electrical_dialog::key(d, &chord) {
                self.cal_action(a, out);
            } else {
                self.swallow_text = swallow;
            }
            return;
        }
        let plain = !(chord.command || chord.alt);
        let action = match chord.key {
            Key::Enter if v.edit.is_some() => v.finish(&st),
            Key::Enter => None,
            Key::ArrowUp => {
                v.move_focus(&st, -1);
                None
            }
            Key::Tab if chord.shift => {
                v.move_focus(&st, -1);
                None
            }
            Key::ArrowDown | Key::Tab => {
                v.move_focus(&st, 1);
                None
            }
            Key::PageUp | Key::PageDown | Key::Home | Key::End if v.edit.is_none() => {
                let d = match chord.key {
                    Key::PageUp => -CAL_PAGE,
                    Key::PageDown => CAL_PAGE,
                    Key::Home => i32::MIN / 2,
                    _ => i32::MAX / 2,
                };
                v.jump_focus(&st, d);
                None
            }
            Key::ArrowLeft | Key::ArrowRight if v.edit.is_none() => {
                v.step_curve(&st, chord.key == Key::ArrowRight)
            }
            _ if v.edit.is_some() => {
                self.swallow_text = swallow;
                None
            }
            Key::N | Key::F2 if plain => {
                if v.start_mic_name(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::I if plain => {
                if v.start_import(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::R if plain => {
                if v.start_rename(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::E if plain => {
                if v.start_electrical(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::C if plain => {
                if v.start_acoustic(&st) {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::Delete | Key::Backspace => v.delete(&st),
            _ => {
                self.swallow_text = swallow;
                None
            }
        };
        if let Some(a) = action {
            self.cal_action(a, out);
        }
    }

    /// A calibrations-view action as requests.
    fn cal_action(&mut self, a: CalAction, out: &mut Vec<Request>) {
        match a {
            CalAction::Inputs(row, what) => out.push(Request::Call {
                what,
                cmd: Command::SessionInputs { inputs: vec![row] },
            }),
            CalAction::Import { path, mic, input } => {
                out.push(Request::ImportCurve { path, mic, input })
            }
            CalAction::Rename(id, label) => out.push(Request::Call {
                what: format!(
                    "curve {} renamed {label}",
                    ac2_scene::cal::curve_name(&id.mic, &id.label)
                ),
                cmd: Command::CalCurveRename { curve: id, label },
            }),
            CalAction::DeleteCurve(id) => out.push(Request::Call {
                what: format!(
                    "curve {} deleted",
                    ac2_scene::cal::curve_name(&id.mic, &id.label)
                ),
                cmd: Command::CalCurveDelete { curve: id },
            }),
            CalAction::Calibrate(cmd, what) => out.push(Request::Call { what, cmd }),
            CalAction::DeleteSensitivity(key) => out.push(Request::Call {
                what: format!(
                    "sensitivity calibration of {} on input {} deleted",
                    key.mic,
                    u32::from(key.channel) + 1
                ),
                cmd: Command::CalDelete { key },
            }),
        }
    }

    fn session_msg(&mut self, m: SessionMsg, out: &mut Vec<Request>) {
        let level = self.stimulus.level;
        let ceiling = self.ceiling();
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        // The mouse reaches a row on the page it is drawn on.
        if let SessionMsg::Focus(r)
        | SessionMsg::Cycle(r, _)
        | SessionMsg::Toggle(r)
        | SessionMsg::Role(r, _)
        | SessionMsg::EditMic(r) = m
        {
            s.show_row(r);
        }
        let d = &mut s.session;
        match m {
            SessionMsg::Focus(r) => {
                s.on_ceiling = false;
                d.detect_cancel();
                d.focus_row(r);
            }
            SessionMsg::Cycle(r, step) => {
                d.detect_cancel();
                d.focus_row(r);
                d.cycle(step, &self.prefs);
            }
            SessionMsg::Toggle(r) => {
                s.on_ceiling = false;
                d.focus_row(r);
                d.toggle();
            }
            SessionMsg::Role(r, k) => {
                s.on_ceiling = false;
                d.focus_row(r);
                d.assign(k);
                if k == crate::session_dialog::RoleKey::Stimulus {
                    self.stimulus_ticks_now(out);
                }
            }
            SessionMsg::EditMic(r) => {
                s.on_ceiling = false;
                d.focus_row(r);
                if matches!(r, Row::Output(_)) {
                    d.start_label_edit();
                } else {
                    d.start_mic_edit();
                }
            }
            SessionMsg::Detect => {
                if let Err(e) = d.detect_start(level) {
                    d.error = Some(e);
                }
            }
            SessionMsg::DetectConfirm => {
                if let Some(req) = d.detect_confirm(ceiling) {
                    out.push(Request::DetectLoopback(req));
                }
            }
            SessionMsg::DetectCancel => d.detect_cancel(),
            SessionMsg::Submit => self.submit_session(out),
            SessionMsg::Cancel => self.overlay = Overlay::None,
        }
    }

    /// Enter on the session dialog: opens the session its roles describe, remembers them
    /// for the device, and points the stimulus at the chosen outputs (K4).
    fn submit_session(&mut self, out: &mut Vec<Request>) {
        let no_measurements = self.daemon().is_some_and(|s| s.measurements.is_empty());
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let d = &mut s.session;
        d.finish_edit();
        let plan = match d.plan() {
            Ok(p) => p,
            Err(e) => {
                d.error = Some(e);
                return;
            }
        };
        let backend = plan
            .config
            .backend
            .unwrap_or(ac2_proto::model::BackendKind::Fake);
        self.prefs.sessions.insert(
            crate::prefs::UiPrefs::device_key(backend, &plan.device_id),
            plan.roles.clone(),
        );
        if !plan.roles.stimulus.is_empty() {
            self.prefs
                .outputs
                .insert(plan.device_id.clone(), plan.roles.stimulus.clone());
            if self.stimulus.phase == StimPhase::Idle {
                self.stimulus.outputs = plan.roles.stimulus.clone();
                self.stim_device = Some(plan.device_id.clone());
            }
        }
        self.prefs_dirty = true;
        out.push(Request::OpenSession {
            config: plan.config,
            inputs: plan.inputs,
            transfers: if no_measurements {
                plan.transfers
            } else {
                Vec::new()
            },
            what: format!("audio session open on {}", plan.device_name),
        });
        self.overlay = Overlay::None;
    }

    /// The new SPL log confirmation: start it, or keep the current log.
    fn new_log(&mut self, go: bool, out: &mut Vec<Request>) {
        let Overlay::NewLog(p) = &self.overlay else {
            return;
        };
        if go {
            let (meas, what) = (p.meas, format!("{}: new SPL log started", p.meter));
            self.call(out, Command::SplLogNew { meas }, what);
        }
        self.overlay = Overlay::None;
    }

    /// The measurement offer: create every transfer measurement, or skip.
    fn offer(&mut self, create: bool, out: &mut Vec<Request>) {
        let Overlay::Offer(o) = &self.overlay else {
            return;
        };
        if create {
            for config in o.transfers.clone() {
                out.push(Request::CreateMeas { config });
            }
        }
        self.overlay = Overlay::None;
    }

    /// X inserts the first arrival, Shift+X the strongest — when the finder accepted. An
    /// ambiguous first arrival is the operator's to pick (decision 1c): the candidate list
    /// opens. A refusal inserts nothing and says why (the banner keeps saying it).
    fn delay_found(
        &mut self,
        meas: MeasId,
        pick: DelayPick,
        finding: DelayFinding,
        out: &mut Vec<Request>,
    ) {
        let name = self.meas(meas).map_or_else(
            || format!("measurement {}", meas.0),
            |m| m.config.name.clone(),
        );
        match (&finding.outcome, pick) {
            (DelayOutcome::NoEstimate { reasons }, _) => self.error(format!(
                "{name}: no delay estimate ({})",
                ac2_scene::finding::no_estimate_reasons(reasons)
            )),
            (DelayOutcome::Ambiguous { .. }, DelayPick::FirstArrival) => {
                self.overlay = Overlay::DelayPick(Box::new(DelayChoice {
                    meas,
                    name,
                    finding,
                    selected: 0,
                }));
            }
            _ => {
                let what = match pick {
                    DelayPick::Strongest => "strongest arrival inserted",
                    _ => "first arrival inserted",
                };
                self.call(
                    out,
                    Command::DelayInsert { meas, pick },
                    format!("{name}: {what}"),
                );
            }
        }
    }

    fn stim_event(&mut self, s: StimEvent, out: &mut Vec<Request>) {
        match s {
            StimEvent::Armed => {
                self.stimulus.phase = StimPhase::Armed;
                if self.armed_with.take() != self.stimulus.settings() {
                    self.resend_stimulus(out);
                }
                let d = self.stimulus.describe();
                if self.sweep.plan.is_some() {
                    self.toast(format!("armed: {d} · Enter plays the sweep · Esc stops"));
                } else {
                    self.toast(format!("armed: {d} · Enter fires · Esc stops"));
                }
            }
            StimEvent::SweepStarted(run) => {
                self.stimulus.phase = StimPhase::Firing;
                self.sweep.run = Some(run.id);
                self.sweep.seen = Some(run.status.clone());
                self.toast(format!(
                    "sweep playing: {} × {} s · Esc stops (and discards it)",
                    run.repeats,
                    format::fixed(run.sweep_duration.0 + run.post_roll.0, 1)
                ));
            }
            StimEvent::Set { firing } => {
                if self.stimulus.phase != StimPhase::Stopping {
                    self.stimulus.phase = if firing {
                        StimPhase::Firing
                    } else {
                        StimPhase::Armed
                    };
                }
            }
            StimEvent::Stopped => {
                self.stimulus.phase = StimPhase::Idle;
                if std::mem::take(&mut self.stimulus.arm_after_stop) && !self.sweep.arm_after_stop {
                    self.arm_after_stopped(out);
                }
                let releasing = std::mem::take(&mut self.sweep.releasing);
                let pending = std::mem::take(&mut self.sweep.arm_after_stop);
                match self.sweep.plan.take() {
                    Some(plan) if pending => self.arm_sweep(plan, false, out),
                    plan => {
                        self.sweep.plan = plan;
                        self.end_sweep_mode();
                        if !releasing {
                            self.toast("stimulus stopped");
                        }
                    }
                }
            }
            StimEvent::Lost(msg) => {
                self.stimulus.phase = StimPhase::Idle;
                self.stimulus.arm_after_stop = false;
                self.sweep.releasing = false;
                self.sweep.arm_after_stop = false;
                self.end_sweep_mode();
                self.error(format!("stimulus lease lost: {msg}"));
            }
            StimEvent::Failed(msg) => {
                self.sweep.releasing = false;
                self.stimulus.arm_after_stop = false;
                if std::mem::take(&mut self.sweep.arm_after_stop) {
                    self.end_sweep_mode();
                }
                self.stimulus.phase = match self.stimulus.phase {
                    StimPhase::Arming => StimPhase::Idle,
                    StimPhase::FireRequested => StimPhase::Armed,
                    // A failed stop leaves the truth to the mirror; the daemon's lease
                    // expiry fades the output out if this client cannot reach it.
                    StimPhase::Stopping => StimPhase::Idle,
                    p => p,
                };
                self.error(format!("stimulus: {msg}"));
            }
        }
    }

    /// M: averages the shown stored transfer traces, phase referred to the phase reference
    /// when it is one of them, else to the first (decision 8b).
    fn average(&mut self, method: AverageMethod, out: &mut Vec<Request>) {
        let shown: Vec<&TraceMeta> = self
            .shown_transfer_traces()
            .into_iter()
            .filter(|t| matches!(t.kind, TraceKind::Transfer | TraceKind::Sweep))
            .collect();
        if shown.len() < 2 {
            self.error("average: show at least two stored transfer traces (1…9)");
            return;
        }
        let ids: Vec<TraceId> = shown.iter().map(|t| t.id).collect();
        let reference = match self.view.tf.phase_reference {
            Some(TraceKey::Stored(r)) if ids.contains(&r) => r,
            _ => ids[0],
        };
        let label = |t: &TraceMeta| {
            t.edit
                .slot
                .map_or_else(|| t.edit.name.clone(), |s| format!("S{s}"))
        };
        let name = format!(
            "avg {}",
            shown.iter().map(|t| label(t)).collect::<Vec<_>>().join("+")
        );
        let m = match method {
            AverageMethod::Power => "power",
            AverageMethod::Complex => "complex",
            AverageMethod::CoherenceWeighted => "coherence-weighted",
        };
        let what = format!("{name} ({m}) created");
        self.call(
            out,
            Command::TraceAverage {
                traces: ids,
                method,
                reference: DelayReference::Trace { trace: reference },
                name,
            },
            what,
        );
    }

    /// Operand `o`'s name: the measurement's or the stored trace's.
    pub fn operand_name(&self, o: Operand) -> String {
        match o {
            Operand::Meas { meas } => self
                .meas(meas)
                .map_or_else(|| format!("measurement {meas}"), |m| m.config.name.clone()),
            Operand::Trace { trace } => self
                .daemon()
                .and_then(|s| s.traces.iter().find(|t| t.id == trace))
                .map_or_else(|| format!("trace {trace}"), |t| t.edit.name.clone()),
        }
    }

    /// What a math channel can combine: the live measurements and the stored traces.
    fn math_candidates(&self) -> Vec<crate::math_dialog::Candidate> {
        crate::math_dialog::Candidate::all(&self.measurements(), &self.trace_list())
    }

    /// Note the spectrum / RTA measurements that started since the last state: each fits
    /// the level axis on its first frame.
    fn follow_spectrum_starts(&mut self) {
        let running: BTreeSet<MeasId> = self
            .measurements()
            .iter()
            .filter(|m| m.running && spectrum_stream(m).is_some())
            .map(|m| m.id)
            .collect();
        for &id in running.difference(&self.spectrum_running) {
            let shown = self.spectrum_frame(id).map(|f| f.frame.clone());
            self.spectrum_fit.insert(id, shown);
        }
        self.spectrum_fit.retain(|id, _| running.contains(id));
        self.spectrum_running = running;
    }

    /// The newest frame of spectrum / RTA measurement `id`.
    pub(super) fn spectrum_frame(&self, id: MeasId) -> Option<&ac2_client::TopicFrame> {
        let m = self.meas(id)?;
        let stream = spectrum_stream(m)?;
        self.data
            .as_ref()?
            .latest
            .get(&Topic::Data { meas: id, stream })
    }

    /// Folds the newest spectrum / RTA frames into the spectrograph histories. A stream
    /// that is STALE, or whose measurement is not running, marks a break: the time until
    /// its next frame is a gap in the picture.
    fn fold_spectrographs(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        use ac2_scene::spectrograph::{SpectrographFrame, SpectrographHistory};
        let span = self.view.spectrum.spectrograph.span_s;
        let running: BTreeMap<MeasId, bool> = self
            .measurements()
            .iter()
            .map(|m| (m.id, m.running))
            .collect();
        self.spectrographs.retain(|id, _| running.contains_key(id));
        for tf in d.latest.frames.values() {
            let (meas, scale, level, validity) = match &tf.frame.data {
                FrameData::Spec(f) => (f.meas, f.meta.scale, &f.level, None),
                FrameData::Rta(f) => (f.meas, f.meta.scale, &f.level, Some(f.validity.as_slice())),
                _ => continue,
            };
            let Some(&live) = running.get(&meas) else {
                continue;
            };
            let Some((grid, def)) = tf
                .frame
                .stamp
                .grid_id
                .and_then(|g| d.grids.get(&g).map(|def| (g, def)))
            else {
                continue;
            };
            let h = self
                .spectrographs
                .entry(meas)
                .or_insert_with(|| SpectrographHistory::new(span));
            if tf.stale || !live {
                h.mark_break();
                continue;
            }
            let cols = ac2_scene::grid::columns(def);
            h.push(&SpectrographFrame {
                seq: tf.frame.stamp.seq,
                at: tf.frame.stamp.capture_wall_ns,
                grid,
                edges: &cols.edges,
                scale,
                level,
                validity,
            });
        }
    }

    fn fold_peaks(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        for tf in d.latest.frames.values() {
            let (meas, level, validity) = match &tf.frame.data {
                FrameData::Spec(f) => (f.meas, &f.level, None),
                FrameData::Rta(f) => (f.meas, &f.level, Some(f.validity.as_slice())),
                _ => continue,
            };
            let seq = tf.frame.stamp.seq;
            let at = tf.frame.stamp.capture_wall_ns.0;
            let e = self
                .peaks
                .entry(meas)
                .or_insert_with(|| (0, at, PeakHold::new(0.0)));
            if e.2.values().is_empty() || seq > e.0 {
                let dt = (at.saturating_sub(e.1)) as f32 / 1e9;
                e.2.update(level, validity, dt);
                e.0 = seq;
                e.1 = at;
            }
        }
    }
}

/// The stream a spectrum / RTA measurement's curve comes on; `None` for other kinds.
pub(crate) fn spectrum_stream(m: &Measurement) -> Option<Stream> {
    m.config
        .kind
        .publishes_levels()
        .then(|| m.config.kind.stream())
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

/// A stored trace as messages and captions name it: `slot 3 (Main L S3)`, or its name.
pub fn trace_label(t: &TraceMeta) -> String {
    match t.edit.slot {
        Some(n) => format!("slot {n} ({})", t.edit.name),
        None => t.edit.name.clone(),
    }
}

/// What a trace command without a selected trace says.
const SELECT_TRACE_FIRST: &str =
    "select a stored trace first (V, Alt+V for hidden ones, or click it in the list)";

/// A display offset as its prompt starts: empty for none, else `-3.5`.
fn offset_text(v: f64) -> String {
    if v == 0.0 {
        String::new()
    } else {
        format::fixed(v, 1).replace(format::MINUS, "-")
    }
}

/// A slot as typed: `1` … `9`, or `none` (also empty, `-`, `off`) to free it.
pub fn parse_slot(text: &str) -> Result<Option<u8>, String> {
    let t = text.trim().to_ascii_lowercase();
    let t = t.strip_prefix("slot").map_or(t.as_str(), str::trim);
    if matches!(t, "" | "none" | "-" | "off") {
        return Ok(None);
    }
    match t.parse::<u8>() {
        Ok(n @ 1..=9) => Ok(Some(n)),
        _ => Err(format!("{text:?}: a slot is 1 … 9, or none")),
    }
}

/// Whether pane `p` draws stored trace `t` (when shown).
fn drawn_in(t: &TraceMeta, p: PaneKind) -> bool {
    match p {
        PaneKind::Transfer | PaneKind::Ir => matches!(
            t.kind,
            TraceKind::Transfer | TraceKind::Target | TraceKind::Sweep
        ),
        PaneKind::Spectrum => matches!(t.kind, TraceKind::Spectrum { .. } | TraceKind::Rta { .. }),
        PaneKind::Distortion => t.kind == TraceKind::Sweep,
        PaneKind::Spl => false,
    }
}

/// A stored trace the transfer pane draws: shown, and a transfer-like curve.
pub fn on_transfer_pane(t: &TraceMeta) -> bool {
    t.edit.visible
        && matches!(
            t.kind,
            TraceKind::Transfer | TraceKind::Target | TraceKind::Sweep
        )
}

pub fn open_session_hint(keymap: &Keymap) -> String {
    let first = |c| {
        keymap
            .chords(c, Scope::Global)
            .first()
            .map(|k: &Chord| k.label())
    };
    match (first(CommandId::OpenSession), first(CommandId::Palette)) {
        (Some(o), Some(p)) => format!("press {o} (or {p} → Open audio session)"),
        (Some(o), None) => format!("press {o}"),
        (None, Some(p)) => format!("{p} → Open audio session"),
        (None, None) => "command palette → Open audio session".into(),
    }
}

fn slot_of(c: CommandId) -> u8 {
    use CommandId as C;
    match c {
        C::Slot1 | C::ShowSlot1 => 1,
        C::Slot2 | C::ShowSlot2 => 2,
        C::Slot3 | C::ShowSlot3 => 3,
        C::Slot4 | C::ShowSlot4 => 4,
        C::Slot5 | C::ShowSlot5 => 5,
        C::Slot6 | C::ShowSlot6 => 6,
        C::Slot7 | C::ShowSlot7 => 7,
        C::Slot8 | C::ShowSlot8 => 8,
        _ => 9,
    }
}

/// A session given by name, or by path (anything with a separator, or starting with `.` or
/// `~`); a path is made absolute, since the daemon does not share this process's working
/// directory.
pub fn parse_session_ref(text: &str) -> Result<SessionRef, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("type a session name or a directory path".into());
    }
    let is_path = t.contains('/')
        || t.contains('\\')
        || t.starts_with('.')
        || t.starts_with('~')
        || std::path::Path::new(t).is_absolute();
    if !is_path {
        return Ok(SessionRef::Name { name: t.to_owned() });
    }
    let p = match t.strip_prefix('~') {
        Some(rest) => std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|h| std::path::PathBuf::from(h).join(rest.trim_start_matches(['/', '\\'])))
            .ok_or_else(|| "no home directory to expand ~".to_string())?,
        None => std::path::PathBuf::from(t),
    };
    let p = std::path::absolute(&p).map_err(|e| e.to_string())?;
    Ok(SessionRef::Path {
        path: p.to_string_lossy().into_owned(),
    })
}

/// A number with an optional unit suffix (case-insensitive); accepts `−` and `,` decimal.
pub fn parse_number(text: &str, units: &[&str]) -> Result<f64, String> {
    let mut t = text.trim().to_ascii_lowercase().replace(format::MINUS, "-");
    for u in units {
        if let Some(s) = t.strip_suffix(u) {
            t = s.trim().to_string();
            break;
        }
    }
    let t = t.replace(',', ".");
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(format!("not a number: {:?}", text.trim())),
    }
}

/// The input a measurement's mic curve belongs to (a transfer function's measurement
/// input); a math channel has its operands', none of its own.
pub fn meas_input(k: &MeasKind) -> Option<u16> {
    match k {
        MeasKind::Transfer { config } => Some(config.measurement_input),
        MeasKind::Spectrum { config } => Some(config.input),
        MeasKind::Rta { config } => Some(config.input),
        MeasKind::Spl { config } => Some(config.input),
        MeasKind::Math { .. } => None,
    }
}

/// The toast of a curve choice: `input 2: mic curve MM1 34804 90°`, `input 2: mic curve off`.
pub fn curve_what(row: &InputSetup) -> String {
    let n = u32::from(row.channel) + 1;
    match (&row.mic, &row.curve) {
        (Some(m), CurveChoice::Curve { label }) => {
            format!(
                "input {n}: mic curve {}",
                ac2_scene::cal::curve_name(m, label)
            )
        }
        (_, CurveChoice::NotChosen) => format!("input {n}: no mic curve chosen"),
        _ => format!("input {n}: mic curve off"),
    }
}

/// `3=M30, 4=ECM` (1-based) of the rows with a mic name, or `3=` for a cleared one.
pub fn mics_text(rows: &[InputSetup]) -> String {
    rows.iter()
        .map(|r| {
            format!(
                "{}={}",
                u32::from(r.channel) + 1,
                r.mic.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parses `3=M30, 4=ECM, 5=` into zero-based channels and names (`None` clears).
pub fn parse_mics(text: &str) -> Result<Vec<(u16, Option<String>)>, String> {
    let mut v: Vec<(u16, Option<String>)> = Vec::new();
    for part in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (ch, name) = part
            .split_once('=')
            .ok_or_else(|| format!("{part:?}: expected input=name, e.g. 3=M30"))?;
        let n: u16 = ch
            .trim()
            .parse()
            .map_err(|_| format!("not an input number: {:?}", ch.trim()))?;
        if n == 0 {
            return Err("inputs count from 1".into());
        }
        let name = name.trim();
        if name.chars().count() > 64 {
            return Err(format!(
                "mic name of input {n} is longer than 64 characters"
            ));
        }
        if v.iter().any(|(c, _)| *c == n - 1) {
            return Err(format!("input {n} given twice"));
        }
        v.push((n - 1, (!name.is_empty()).then(|| name.to_owned())));
    }
    if v.is_empty() {
        return Err("type at least one input=name".into());
    }
    Ok(v)
}

/// `80-800`, `80 – 800 Hz`: band edges in Hz, lower first.
pub fn parse_band(text: &str) -> Result<(f64, f64), String> {
    let t = text.replace(['–', '—'], "-");
    let (a, b) = t
        .split_once('-')
        .ok_or_else(|| format!("{:?}: expected low-high, e.g. 80-800", text.trim()))?;
    let lo = parse_number(a, &["hz"])?;
    let hi = parse_number(b, &["hz"])?;
    if !(lo > 0.0 && hi > lo) {
        return Err("band edges must be above 0 Hz, the upper above the lower".into());
    }
    Ok((lo, hi))
}

/// `1, 2` (one-based) → `[0, 1]`.
pub fn parse_outputs(text: &str) -> Result<Vec<u16>, String> {
    let mut v = Vec::new();
    for part in text.split([',', ' ']).filter(|s| !s.is_empty()) {
        let n: u16 = part
            .parse()
            .map_err(|_| format!("not a channel number: {part:?}"))?;
        if n == 0 {
            return Err("channels count from 1".into());
        }
        if !v.contains(&(n - 1)) {
            v.push(n - 1);
        }
    }
    if v.is_empty() {
        return Err("at least one output channel".into());
    }
    Ok(v)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
