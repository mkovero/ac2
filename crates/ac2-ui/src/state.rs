//! Application state and its reducer.
//!
//! [`AppState::update`] is pure: a message in, state changed, [`Request`]s for the daemon
//! link out. No clock, no socket, no egui — every keyboard flow (including the stimulus
//! arm → fire → stop cluster) is tested here without a window.
//!
//! The UI computes no measurement values: it keeps operator choices (view ranges, display
//! offsets, polarity, nudges, selection) and hands them with the received frames to
//! `ac2-scene`, which does all display math.

use std::collections::BTreeMap;
use std::sync::Arc;

use ac2_client::MirrorView;
use ac2_proto::Command;
use ac2_proto::GridDef;
use ac2_proto::model::{
    AverageMethod, CalKey, CalPart, DelayFinding, DelayOutcome, DelayPick, DelayReference,
    FinderBand, GeneratorDesired, GeneratorSettings, ImportRole, InputSetup, MathOp, MeasKind,
    Measurement, SessionRef, Signal, Smoothing, SmoothingFraction, SmoothingMode, State, TraceData,
    TraceKind, TraceMeta,
};
use ac2_proto::units::{ClientId, Dbfs, Hz, MeasId, Seconds, TraceId};
use ac2_scene::spectrum::PeakHold;
use ac2_scene::theme::ThemeName;
use ac2_scene::trace::TraceKey;
use ac2_scene::view::{CoherencePlacement, FreqRange, IrMode, PhaseView, SpectrumStyle, ViewState};
use ac2_scene::{axis::Range, format};

use crate::anim::FreqNav;
use crate::conn::{ConnEvent, DataSnapshot, Request, StimEvent};
use crate::forms::{Form, FormKind};
use crate::keys::{Chord, CommandId, Keymap, Scope};
use crate::palette::Palette;
use crate::prefs::UiPrefs;
use crate::session_dialog::{Edit, RoleKey, Row, SessionDialog};

/// The four panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneKind {
    Transfer,
    Spectrum,
    Ir,
    Spl,
}

impl PaneKind {
    pub const ALL: [PaneKind; 4] = [
        PaneKind::Transfer,
        PaneKind::Spectrum,
        PaneKind::Ir,
        PaneKind::Spl,
    ];

    pub fn scope(self) -> Scope {
        match self {
            PaneKind::Transfer => Scope::Transfer,
            PaneKind::Spectrum => Scope::Spectrum,
            PaneKind::Ir => Scope::Ir,
            PaneKind::Spl => Scope::Spl,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            PaneKind::Transfer => "Transfer",
            PaneKind::Spectrum => "Spectrum / RTA",
            PaneKind::Ir => "Impulse response",
            PaneKind::Spl => "SPL",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    /// Whether the pane shows measurements of kind `k` (the IR pane shows the transfer
    /// pane's measurement).
    pub fn shows(self, k: &MeasKind) -> bool {
        match self {
            PaneKind::Transfer | PaneKind::Ir => matches!(k, MeasKind::Transfer { .. }),
            PaneKind::Spectrum => matches!(k, MeasKind::Spectrum { .. } | MeasKind::Rta { .. }),
            PaneKind::Spl => matches!(k, MeasKind::Spl { .. }),
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
        match k {
            MeasKind::Transfer { .. } => PaneKind::Transfer,
            MeasKind::Spectrum { .. } | MeasKind::Rta { .. } => PaneKind::Spectrum,
            MeasKind::Spl { .. } => PaneKind::Spl,
        }
    }

    /// What the pane's measurements are, for messages.
    pub fn what(self) -> &'static str {
        match self {
            PaneKind::Transfer | PaneKind::Ir => "transfer",
            PaneKind::Spectrum => "spectrum or RTA",
            PaneKind::Spl => "SPL",
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

/// What the smoothing keys change: the selected slot's trace, or the transfer pane's
/// measurement.
#[derive(Clone, Debug, PartialEq)]
pub enum SmoothTarget {
    Trace(TraceMeta),
    Meas(Measurement),
}

impl SmoothTarget {
    pub fn smoothing(&self) -> Option<Smoothing> {
        match self {
            SmoothTarget::Trace(t) => t.edit.smoothing,
            SmoothTarget::Meas(m) => match &m.config.kind {
                MeasKind::Transfer { config } => config.smoothing,
                _ => None,
            },
        }
    }

    /// `slot 3 (Main L S3)` or the measurement name.
    pub fn label(&self) -> String {
        match self {
            SmoothTarget::Trace(t) => match t.edit.slot {
                Some(n) => format!("slot {n} ({})", t.edit.name),
                None => t.edit.name.clone(),
            },
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
    pub shown: [bool; 4],
    /// Only the focused pane.
    pub maximized: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            focus: PaneKind::Transfer,
            shown: [true; 4],
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
}

impl Default for Stimulus {
    fn default() -> Self {
        Self {
            level: None,
            signal: Signal::Pink,
            outputs: vec![0],
            phase: StimPhase::Idle,
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
            Signal::Ess { .. } => "sweep".to_string(),
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

/// What a text prompt sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    StimulusLevel,
    StimulusOutputs,
    Offset(MeasId),
    Delay(MeasId),
    /// A file to import as a trace or target curve.
    ImportFile(ImportRole),
    SessionSave,
    SessionLoad,
    InputMics,
    /// `input=mic` of a calibration on the session's capture device to delete from.
    CalDelete(CalPart),
    /// Custom delay-finder band edges.
    FinderBand,
    /// Delay-finder observation.
    FinderObservation,
}

impl PromptKind {
    pub fn label(self) -> &'static str {
        match self {
            PromptKind::StimulusLevel => "Stimulus level (dBFS)",
            PromptKind::StimulusOutputs => "Stimulus outputs (1-based, e.g. 1, 2)",
            PromptKind::Offset(_) => "Display offset (dB)",
            PromptKind::Delay(_) => "Delay (ms)",
            PromptKind::ImportFile(ImportRole::Target) => "Target curve file (path)",
            PromptKind::ImportFile(ImportRole::Trace) => "Trace file to import (path)",
            PromptKind::SessionSave => "Save session as (name or path)",
            PromptKind::SessionLoad => "Load session, disarmed (name or path)",
            PromptKind::InputMics => "Mic per input (1-based, e.g. 3=M30, 4=ECM; 3= clears)",
            PromptKind::CalDelete(CalPart::All) => {
                "Delete calibration and mic curve of input=mic on this device (e.g. 3=M30)"
            }
            PromptKind::CalDelete(CalPart::Sensitivity) => {
                "Delete sensitivity calibration of input=mic on this device (e.g. 3=M30)"
            }
            PromptKind::CalDelete(CalPart::MicCurve) => {
                "Delete mic curve of input=mic on this device (e.g. 3=M30)"
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
    /// Candidate list of an ambiguous finding over the transfer pane. Keys other than 1–3
    /// keep working; Esc closes it (and stops the stimulus, as always).
    DelayPick(Box<DelayChoice>),
    /// A new-measurement dialog.
    Form(Box<Form>),
    /// The audio session dialog.
    Session(Box<SessionDialog>),
    /// A pane's measurement list, opened from its title chip.
    PaneMenu(PaneMenu),
    /// After a session opened with a reference and mics on a daemon without measurements:
    /// one key creates a transfer measurement per mic.
    Offer(Box<Offer>),
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
    Command(CommandId),
    Conn(Box<ConnEvent>),
    /// Frame tick: `now_s` monotonic seconds, `dt_s` since the previous tick.
    Tick {
        now_s: f64,
        dt_s: f64,
    },
    /// A measurement clicked in the list: selected, and shown by its pane.
    SelectMeas(MeasId),
    /// A slot clicked in the list: selected for the smoothing keys (again: deselected).
    SelectTrace(TraceId),
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
    /// Mouse on an open dialog.
    Form(FormMsg),
    /// Mouse on the session dialog.
    Session(SessionMsg),
    /// Mouse on the measurement offer: `true` creates, `false` skips.
    Offer(bool),
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

/// Everything the UI holds.
#[derive(Clone, Debug)]
pub struct AppState {
    pub conn: ConnState,
    pub mirror: Option<Arc<MirrorView>>,
    pub data: Option<Arc<DataSnapshot>>,
    /// Stored traces' data, as fetched.
    pub traces: BTreeMap<TraceId, (Arc<TraceData>, Arc<GridDef>)>,
    pub theme: ThemeName,
    /// What the scene builders get; `view.freq` follows `nav`.
    pub view: ViewState,
    pub nav: FreqNav,
    pub layout: Layout,
    pub selected: Option<MeasId>,
    /// The measurement each pane shows (keys act on it); a pane without a choice shows the
    /// selected measurement if it fits, else its first one.
    pub pane_meas: BTreeMap<PaneKind, MeasId>,
    /// A slot's trace selected in the list: the smoothing keys change it instead of the
    /// transfer pane's measurement.
    pub selected_trace: Option<TraceId>,
    pub edits: BTreeMap<MeasId, LiveEdit>,
    /// Peak hold per spectrum / RTA measurement, with the last folded-in `seq` and capture
    /// time.
    pub peaks: BTreeMap<MeasId, (u64, u64, PeakHold)>,
    pub stimulus: Stimulus,
    /// Remembered between runs (stimulus outputs per device).
    pub prefs: UiPrefs,
    /// `prefs` changed since the app last saved them.
    pub prefs_dirty: bool,
    /// The output device the stimulus outputs belong to (the open session's).
    stim_device: Option<String>,
    /// Band and observation X / Shift+X run the finder with.
    pub finder: FinderChoice,
    pub overlay: Overlay,
    pub toasts: Vec<Toast>,
    pub now_s: f64,
    pub quit: bool,
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
            stimulus: Stimulus::default(),
            prefs: UiPrefs::default(),
            prefs_dirty: false,
            stim_device: None,
            finder: FinderChoice::default(),
            overlay: Overlay::None,
            toasts: Vec::new(),
            now_s: 0.0,
            quit: false,
            swallow_text: None,
            armed_with: None,
            pending_select: None,
            open_session_when_empty: false,
            devices: None,
            preview_sent_s: f64::NEG_INFINITY,
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

    /// The stored trace selected in the list, if it still exists.
    pub fn selected_trace_meta(&self) -> Option<&TraceMeta> {
        let id = self.selected_trace?;
        self.daemon()?.traces.iter().find(|t| t.id == id)
    }

    /// What K / Shift+K change now.
    pub fn smooth_target(&self) -> Option<SmoothTarget> {
        match self.selected_trace_meta() {
            Some(t) => Some(SmoothTarget::Trace(t.clone())),
            None => self
                .pane_meas(PaneKind::Transfer)
                .map(|m| SmoothTarget::Meas(m.clone())),
        }
    }

    /// The transfer pane's title caption: what the smoothing keys act on and its setting,
    /// `smoothing 1/6 oct` (the pane's measurement) or `slot 3 (Main L S3): smoothing off`.
    pub fn smoothing_caption(&self) -> Option<String> {
        let t = self.smooth_target()?;
        let c = ac2_scene::tf::smoothing_caption(t.smoothing());
        Some(match t {
            SmoothTarget::Trace(_) => format!("{}: {c}", t.label()),
            SmoothTarget::Meas(_) => c,
        })
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
            .filter(|t| t.edit.visible && matches!(t.kind, TraceKind::Transfer | TraceKind::Target))
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
    pub fn empty_hint(&self, keymap: &Keymap) -> Option<String> {
        if !self.connected() {
            return None;
        }
        let st = self.daemon()?;
        if st.session.open.is_none() {
            return Some(format!("No audio session — {}", open_session_hint(keymap)));
        }
        if st.measurements.is_empty() {
            let palette = keymap
                .chords(CommandId::Palette, Scope::Global)
                .first()
                .map_or_else(|| "Command palette".to_owned(), |c| c.label());
            return Some(format!(
                "No measurements — {palette} → New transfer measurement… (or New spectrum, RTA, SPL meter)"
            ));
        }
        None
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
        self.sync_meters(before, tick, &mut out);
        out
    }

    /// What the open dialog needs metered: the input meters subscription, and a device
    /// preview (the session dialog on a device the session does not capture).
    fn meter_wants(
        &self,
    ) -> (
        bool,
        Option<(ac2_proto::model::BackendKind, ac2_proto::model::DeviceId)>,
    ) {
        match &self.overlay {
            Overlay::Session(d) => (true, d.preview_target()),
            Overlay::Form(_) => (true, None),
            _ => (false, None),
        }
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
        let from_preview = match &self.overlay {
            Overlay::Session(s) => s.preview_target(),
            _ => None,
        };
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

    /// The names of the open session's inputs, as the dialogs show them: mic name, else the
    /// device's channel name, else `Input N`.
    pub fn session_input_names(&self) -> Vec<(u16, String)> {
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
                let name = ac2_scene::meter::input_name(c, mic.as_deref(), dev.as_deref());
                (c, ac2_scene::meter::channel_choice(c, &name))
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
                Overlay::Session(d) => d.backspace(),
                Overlay::Prompt(p) => {
                    p.text.pop();
                    p.error = None;
                }
                _ => {}
            },
            Msg::Command(c) => self.command(c, keymap, out),
            Msg::Conn(e) => self.conn_event(*e, keymap, out),
            Msg::Form(m) => self.form_msg(m, out),
            Msg::Session(m) => self.session_msg(m, out),
            Msg::Offer(create) => self.offer(create, out),
            Msg::Tick { now_s, dt_s } => {
                self.now_s = now_s;
                self.nav.step(dt_s);
                self.view.freq = self.nav.current();
                let now = self.now_s;
                self.toasts.retain(|t| t.until_s > now);
            }
            Msg::SelectMeas(id) => self.select(id),
            Msg::SelectTrace(id) => {
                self.selected_trace = if self.selected_trace == Some(id) {
                    None
                } else {
                    Some(id)
                };
            }
            Msg::FocusPane(p) => self.focus(p),
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
        // Esc always stops, whatever is open; it also closes the overlay.
        if chord == Chord::key(Key::Escape) {
            self.overlay = Overlay::None;
            self.command(CommandId::StimulusStop, keymap, out);
            return;
        }
        match &mut self.overlay {
            Overlay::Palette(p) => {
                let scope = self.layout.focus.scope();
                match chord.key {
                    Key::ArrowDown | Key::ArrowUp => {
                        let n = p.entries(keymap, scope).len();
                        p.move_by(if chord.key == Key::ArrowDown { 1 } else { -1 }, n);
                    }
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
            Overlay::Session(_) => {
                self.session_key(chord, swallow, out);
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
            Overlay::Form(f) => {
                match chord.key {
                    Key::Enter => self.submit_form(out),
                    Key::ArrowUp => f.move_focus(-1),
                    Key::Tab if chord.shift => f.move_focus(-1),
                    Key::ArrowDown | Key::Tab => f.move_focus(1),
                    Key::ArrowLeft => f.cycle(-1),
                    Key::ArrowRight => f.cycle(1),
                    _ => self.swallow_text = swallow,
                }
                return;
            }
            Overlay::DelayPick(choice) => {
                let index = match chord {
                    c if c == Chord::key(Key::Num1) => Some(0u8),
                    c if c == Chord::key(Key::Num2) => Some(1),
                    c if c == Chord::key(Key::Num3) => Some(2),
                    _ => None,
                };
                if let Some(index) = index
                    && usize::from(index) < choice.rows().len()
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
            Overlay::Help | Overlay::None => {}
        }
        if let Some(c) = keymap.lookup(self.scope(), chord) {
            let before = std::mem::discriminant(&self.overlay);
            self.command(c, keymap, out);
            let opened_text = matches!(
                self.overlay,
                Overlay::Palette(_) | Overlay::Prompt(_) | Overlay::Form(_) | Overlay::Session(_)
            );
            if opened_text && std::mem::discriminant(&self.overlay) != before {
                self.swallow_text = typed_char(&chord);
            }
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
            Overlay::Session(d) => d.type_text(&t),
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
            PromptKind::StimulusOutputs => parse_outputs(&text).map(|o| {
                if let Some(dev) = &self.stim_device {
                    self.prefs.outputs.insert(dev.clone(), o.clone());
                    self.prefs_dirty = true;
                }
                self.stimulus.outputs = o;
                self.resend_stimulus(out);
            }),
            PromptKind::CalDelete(part) => self.cal_delete(&text, part, out),
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
            PromptKind::Offset(id) => parse_number(&text, &["db"]).map(|v| {
                self.edits.entry(id).or_default().offset_db = v;
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
            PromptKind::InputMics => parse_mics(&text).map(|mics| {
                let inputs: Vec<InputSetup> = mics
                    .into_iter()
                    .map(|(channel, mic)| {
                        let mut row = self.input_setup(channel);
                        row.mic = mic;
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
                    what: format!("delay {} ms", format::fixed(v, 2)),
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
        if !matches!(self.stimulus.phase, StimPhase::Idle) && !force {
            return;
        }
        if let Some(settings) = self.stimulus.settings() {
            self.stimulus.phase = StimPhase::Arming;
            self.armed_with = Some(settings.clone());
            out.push(Request::StimArm { settings, force });
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
            .map(|m| {
                let input = meas_input(&m.config.kind);
                let mic = self.input_setup(input).mic.unwrap_or_default();
                format!("{}={mic}", u32::from(input) + 1)
            })
            .unwrap_or_default()
    }

    fn cal_delete(
        &mut self,
        text: &str,
        part: CalPart,
        out: &mut Vec<Request>,
    ) -> Result<(), String> {
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
        let what = match part {
            CalPart::All => "calibration",
            CalPart::Sensitivity => "sensitivity calibration",
            CalPart::MicCurve => "mic curve",
        };
        out.push(Request::Call {
            what: format!(
                "{what} of {mic} on input {} deleted",
                u32::from(channel) + 1
            ),
            cmd: Command::CalDelete {
                key: CalKey {
                    device,
                    channel,
                    mic,
                },
                part,
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

    /// The daemon's input setup row of `channel` (default: no mic name, curve on).
    fn input_setup(&self, channel: u16) -> InputSetup {
        self.daemon()
            .and_then(|s| s.inputs.iter().find(|i| i.channel == channel).cloned())
            .unwrap_or(InputSetup {
                channel,
                mic: None,
                mic_curve: true,
            })
    }

    fn need_tf(&mut self) -> Option<Measurement> {
        self.need_meas(
            &[|k| matches!(k, MeasKind::Transfer { .. })],
            "transfer-function",
        )
    }

    /// Focuses `p` and selects the measurement it shows.
    fn focus(&mut self, p: PaneKind) {
        self.layout.shown[p.index()] = true;
        self.layout.focus = p;
        self.select_shown(p);
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
            self.error("no transfer measurement to smooth");
            return;
        };
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
                        "{}: {} is the {}",
                        target.label(),
                        ac2_scene::tf::smoothing_caption(cur),
                        if step > 0 { "widest" } else { "finest" }
                    ));
                    return;
                }
                SMOOTHING_STEPS[j as usize]
            }
        };
        let new = want.map(|fraction| Smoothing {
            fraction,
            mode: cur.map_or(SmoothingMode::Power, |s| s.mode),
        });
        let what = format!(
            "{}: {}",
            target.label(),
            ac2_scene::tf::smoothing_caption(new)
        );
        match target {
            SmoothTarget::Trace(t) => {
                let label = SmoothTarget::Trace(t.clone()).label();
                if !matches!(t.kind, TraceKind::Transfer) {
                    self.error(format!(
                        "{label}: smoothing applies to transfer traces only"
                    ));
                    return;
                }
                if t.edit.locked {
                    self.error(format!("{label} is locked"));
                    return;
                }
                let mut edit = t.edit.clone();
                edit.smoothing = new;
                self.call(out, Command::TraceUpdate { trace: t.id, edit }, what);
            }
            SmoothTarget::Meas(m) => {
                let mut config = m.config.clone();
                if let MeasKind::Transfer { config: tf } = &mut config.kind {
                    tf.smoothing = new;
                }
                self.call(out, Command::MeasUpdate { meas: m.id, config }, what);
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
        self.layout.focus = vis[((i + d).rem_euclid(n)) as usize];
        self.select_shown(self.layout.focus);
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

            C::StimulusArm => self.arm(false, keymap, out),
            C::StimulusTakeOver => self.arm(true, keymap, out),
            C::StimulusFire => match self.stimulus.phase {
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
            C::StimulusStop => {
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
                let text = outputs_text(&self.stimulus.outputs);
                self.prompt(PromptKind::StimulusOutputs, text);
            }

            C::FocusTransfer => self.focus(PaneKind::Transfer),
            C::FocusSpectrum => self.focus(PaneKind::Spectrum),
            C::FocusIr => self.focus(PaneKind::Ir),
            C::FocusSpl => self.focus(PaneKind::Spl),
            C::NextPane => self.cycle_pane(1),
            C::PrevPane => self.cycle_pane(-1),
            C::MaximizePane => self.layout.maximized = !self.layout.maximized,
            C::NextMeasurement => self.cycle_meas(1),
            C::PrevMeasurement => self.cycle_meas(-1),
            C::PaneMeasurement => self.overlay = self.pane_menu(self.layout.focus),
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
            C::ToggleCursor => {
                let t = self.nav.target;
                self.view.cursor_hz = match self.view.cursor_hz {
                    Some(_) => None,
                    None => Some((t.lo * t.hi).sqrt()),
                };
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
                    &[
                        |k| matches!(k, MeasKind::Transfer { .. }),
                        |k| matches!(k, MeasKind::Spectrum { .. } | MeasKind::Rta { .. }),
                    ],
                    "transfer, spectrum or RTA",
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
                match self.slots()[usize::from(slot - 1)].cloned() {
                    None => self.error(format!(
                        "slot {slot} is empty (Ctrl+{slot} captures into it)"
                    )),
                    Some(t) => {
                        let mut edit = t.edit.clone();
                        edit.visible = !edit.visible;
                        let what = format!(
                            "slot {slot} {}",
                            if edit.visible { "shown" } else { "hidden" }
                        );
                        self.call(out, Command::TraceUpdate { trace: t.id, edit }, what);
                    }
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
                    let open = self.open_session().cloned();
                    let setup = self.daemon().map(|s| s.inputs.clone()).unwrap_or_default();
                    self.overlay =
                        Overlay::Session(Box::new(SessionDialog::new(open.as_ref(), &setup)));
                    out.push(Request::Devices);
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
            C::CalDelete | C::CalDeleteSensitivity | C::CalDeleteCurve => {
                let part = match c {
                    C::CalDeleteSensitivity => CalPart::Sensitivity,
                    C::CalDeleteCurve => CalPart::MicCurve,
                    _ => CalPart::All,
                };
                let text = self.cal_delete_text();
                self.prompt(PromptKind::CalDelete(part), text);
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
                ) {
                    let input = meas_input(&m.config.kind);
                    let mut row = self.input_setup(input);
                    row.mic_curve = !row.mic_curve;
                    let what = format!(
                        "mic curve {} on input {}",
                        if row.mic_curve { "on" } else { "off" },
                        u32::from(input) + 1
                    );
                    if row.mic.is_none() {
                        self.toast(format!(
                            "input {} has no mic name: a curve applies once one is set",
                            u32::from(input) + 1
                        ));
                    }
                    self.call(out, Command::SessionInputs { inputs: vec![row] }, what);
                }
            }

            C::Freeze => {
                if let Some(m) = self.need_meas(
                    &[
                        |k| matches!(k, MeasKind::Transfer { .. }),
                        |k| matches!(k, MeasKind::Spectrum { .. } | MeasKind::Rta { .. }),
                    ],
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
                if let Some(m) = self.selected_meas().cloned() {
                    self.call(
                        out,
                        Command::MeasReset { meas: m.id },
                        format!("{} averaging reset", m.config.name),
                    );
                } else {
                    self.error("select a measurement first (N)");
                }
            }
            C::StartStop => {
                if let Some(m) = self.selected_meas().cloned() {
                    let (cmd, what) = if m.running {
                        (Command::MeasStop { meas: m.id }, "stopped")
                    } else {
                        (Command::MeasStart { meas: m.id }, "started")
                    };
                    self.call(out, cmd, format!("{} {what}", m.config.name));
                } else {
                    self.error("select a measurement first (N)");
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
                        .map(|d| format::fixed(d.applied.0 * 1000.0, 2))
                        .unwrap_or_default();
                    self.prompt(PromptKind::Delay(m.id), text);
                }
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
            C::Invert => {
                if let Some(m) = self.need_tf() {
                    let e = self.edits.entry(m.id).or_default();
                    e.inverted = !e.inverted;
                }
            }
            C::Offset => {
                if let Some(m) = self.need_tf() {
                    let v = self.edit(m.id).offset_db;
                    let text = if v == 0.0 {
                        String::new()
                    } else {
                        format::fixed(v, 1).replace(format::MINUS, "-")
                    };
                    self.prompt(PromptKind::Offset(m.id), text);
                }
            }
            C::NudgeEarlier | C::NudgeLater => {
                if let Some(m) = self.need_tf() {
                    let d = if c == C::NudgeEarlier {
                        -NUDGE_S
                    } else {
                        NUDGE_S
                    };
                    let e = self.edits.entry(m.id).or_default();
                    // Whole steps: repeated nudges never accumulate float error.
                    e.nudge_s = ((e.nudge_s + d) / NUDGE_S).round() * NUDGE_S;
                }
            }
            C::PhaseReference => {
                if let Some(m) = self.need_tf() {
                    self.view.tf.phase_reference = Some(TraceKey::Live(m.id));
                    self.toast(format!("phase reference: {}", m.config.name));
                }
            }
            C::Target => self.prompt(PromptKind::ImportFile(ImportRole::Target), String::new()),
            C::Average => self.average(AverageMethod::Power, out),
            C::AverageComplex => self.average(AverageMethod::Complex, out),
            C::AverageCoherence => self.average(AverageMethod::CoherenceWeighted, out),
            C::MathDifference => self.math(MathOp::MagnitudeDifference, out),
            C::MathDivide => self.math(MathOp::ComplexDivision, out),
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
            C::IrMode => {
                self.view.ir.mode = match self.view.ir.mode {
                    IrMode::Linear => IrMode::Log,
                    IrMode::Log => IrMode::Etc,
                    IrMode::Etc => IrMode::Linear,
                };
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
            }
            ConnEvent::Failed { target, error, .. } => {
                self.conn = ConnState::Failed { target, error };
                self.pending_select = None;
                self.mirror = None;
                self.data = None;
                self.stimulus.phase = StimPhase::Idle;
            }
            ConnEvent::Mirror(v) => {
                self.mirror = Some(v);
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
                self.follow_output_device();
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
                    (Overlay::Session(s), Ok(d)) => s.set_backends(d, &self.prefs),
                    (Overlay::Session(s), Err(e)) => {
                        s.error = Some(format!("cannot list devices: {e}"));
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
                if let Overlay::Session(s) = &mut self.overlay
                    && s.preview_target() == Some((backend, device))
                {
                    s.preview_error = result.err().map(|e| format!("meters unavailable: {e}"));
                }
            }
            ConnEvent::LoopbackDetected(r) => {
                if let Overlay::Session(s) = &mut self.overlay {
                    s.detect_result(r);
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
            ConnEvent::MeasCreated(m) => {
                self.selected = Some(m.id);
                self.pending_select = Some(m.id);
            }
            ConnEvent::Data(d) => {
                if self.view.spectrum.peak_hold {
                    self.fold_peaks(&d);
                }
                self.data = Some(d);
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
            }
            ConnEvent::Reply { what, result } => match result {
                Ok(()) => self.toast(what),
                Err(e) => self.error(format!("{what}: {e}")),
            },
            ConnEvent::DelayFound {
                meas,
                pick,
                finding,
            } => self.delay_found(meas, pick, *finding, out),
            ConnEvent::Captured { slot, trace } => {
                self.toast(format!("slot {slot}: {} captured", trace.edit.name));
            }
            ConnEvent::Stimulus(s) => self.stim_event(s, out),
        }
    }

    fn form_msg(&mut self, m: FormMsg, out: &mut Vec<Request>) {
        match m {
            FormMsg::Submit => self.submit_form(out),
            FormMsg::Cancel => {
                if matches!(self.overlay, Overlay::Form(_)) {
                    self.overlay = Overlay::None;
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
        let Overlay::Form(f) = &mut self.overlay else {
            return;
        };
        let r = f
            .meas_config(open.as_ref())
            .map(|config| Request::CreateMeas { config });
        match r {
            Ok(req) => {
                out.push(req);
                self.overlay = Overlay::None;
            }
            Err(e) => f.error = Some(e),
        }
    }

    /// Keys on the session dialog. Esc never gets here: it closes the dialog and stops the
    /// stimulus like everywhere else.
    fn session_key(&mut self, chord: Chord, swallow: Option<char>, out: &mut Vec<Request>) {
        use eframe::egui::Key;
        let Overlay::Session(d) = &mut self.overlay else {
            return;
        };
        let plain = !(chord.command || chord.alt);
        match chord.key {
            Key::Enter => {
                if d.edit == Some(Edit::DetectLevel) {
                    self.session_msg(SessionMsg::DetectConfirm, out);
                } else if matches!(d.edit, Some(Edit::Mic(_))) {
                    d.finish_edit();
                } else {
                    self.submit_session(out);
                }
            }
            Key::ArrowUp => {
                d.detect_cancel();
                d.move_focus(-1);
            }
            Key::Tab if chord.shift => {
                d.detect_cancel();
                d.move_focus(-1);
            }
            Key::ArrowDown | Key::Tab => {
                d.detect_cancel();
                d.move_focus(1);
            }
            Key::ArrowLeft | Key::ArrowRight if d.edit.is_none() => {
                let step = if chord.key == Key::ArrowLeft { -1 } else { 1 };
                d.cycle(step, &self.prefs);
            }
            // Everything below types text while a text row or edit has the keyboard.
            _ if d.text_focus() => self.swallow_text = swallow,
            Key::Space if plain => d.toggle(),
            Key::R if plain => d.assign(RoleKey::Reference),
            Key::M if plain => d.assign(RoleKey::Mic),
            Key::S if plain => d.assign(RoleKey::Stimulus),
            Key::N | Key::F2 if plain => {
                if d.start_mic_edit() {
                    // The N that started the edit also arrives as text.
                    self.swallow_text = typed_char(&chord);
                }
            }
            Key::D if plain => {
                self.session_msg(SessionMsg::Detect, out);
                if let Overlay::Session(d) = &self.overlay
                    && d.edit == Some(Edit::DetectLevel)
                {
                    self.swallow_text = typed_char(&chord);
                }
            }
            _ => self.swallow_text = swallow,
        }
    }

    fn session_msg(&mut self, m: SessionMsg, out: &mut Vec<Request>) {
        let level = self.stimulus.level;
        let ceiling = self.ceiling();
        let Overlay::Session(d) = &mut self.overlay else {
            return;
        };
        match m {
            SessionMsg::Focus(r) => {
                d.detect_cancel();
                d.focus_row(r);
            }
            SessionMsg::Cycle(r, step) => {
                d.detect_cancel();
                d.focus_row(r);
                d.cycle(step, &self.prefs);
            }
            SessionMsg::Toggle(r) => {
                d.focus_row(r);
                d.toggle();
            }
            SessionMsg::Role(r, k) => {
                d.focus_row(r);
                d.assign(k);
            }
            SessionMsg::EditMic(r) => {
                d.focus_row(r);
                d.start_mic_edit();
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
        let Overlay::Session(d) = &mut self.overlay else {
            return;
        };
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
                self.toast(format!("armed: {d} · Enter fires · Esc stops"));
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
                self.toast("stimulus stopped");
            }
            StimEvent::Lost(msg) => {
                self.stimulus.phase = StimPhase::Idle;
                self.error(format!("stimulus lease lost: {msg}"));
            }
            StimEvent::Failed(msg) => {
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
            .filter(|t| t.kind == TraceKind::Transfer)
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

    /// A − B (or A / B) of the two lowest shown slots.
    fn math(&mut self, op: MathOp, out: &mut Vec<Request>) {
        let slotted: Vec<&TraceMeta> = self
            .shown_transfer_traces()
            .into_iter()
            .filter(|t| t.edit.slot.is_some())
            .collect();
        let [a, b, ..] = slotted.as_slice() else {
            self.error("A − B: show two slotted traces (1…9); the lower slot is A");
            return;
        };
        let (sa, sb) = (a.edit.slot.unwrap_or(0), b.edit.slot.unwrap_or(0));
        let name = match op {
            MathOp::MagnitudeDifference => format!("S{sa} − S{sb}"),
            MathOp::ComplexDivision => format!("S{sa} / S{sb}"),
        };
        let cmd = Command::TraceMath {
            a: a.id,
            b: b.id,
            op,
            name: name.clone(),
        };
        self.call(out, cmd, format!("{name} created"));
    }

    fn fold_peaks(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        for tf in d.latest.frames.values() {
            let (meas, level, validity) = match &tf.frame.data {
                FrameData::Spec(f) => (f.meas, &f.level, &f.validity),
                FrameData::Rta(f) => (f.meas, &f.level, &f.validity),
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
                e.2.update(level, Some(validity), dt);
                e.0 = seq;
                e.1 = at;
            }
        }
    }
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

/// Renames the channel choices of a measurement dialog (the device's channel names arrived).
fn relabel(f: &mut Form, names: &[(u16, String)]) {
    for field in &mut f.fields {
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

/// The input a measurement's mic curve belongs to (a transfer function's measurement input).
pub fn meas_input(k: &MeasKind) -> u16 {
    match k {
        MeasKind::Transfer { config } => config.measurement_input,
        MeasKind::Spectrum { config } => config.input,
        MeasKind::Rta { config } => config.input,
        MeasKind::Spl { config } => config.input,
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
