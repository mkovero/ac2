//! Application state and its reducer.
//!
//! [`AppState::update`] is pure: a message in, state changed, [`Request`]s for the daemon
//! link out. No clock, no socket, no egui — every keyboard flow (including the stimulus
//! arm → fire → stop cluster) is tested here without a window.
//!
//! The UI computes no measurement values: it keeps operator choices (view ranges, display
//! offsets, polarity, nudges, selection) and hands them with the received frames to
//! `ac2-scene`, which does all display math.

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
use ac2_scene::format;
// The reducer tests read these through `super::*`.
use ac2_scene::spectrum::PeakHold;
use ac2_scene::stimulus::{Drive, Next as NextKey, Stimulus as NextStimulus};
use ac2_scene::theme::ThemeName;
use ac2_scene::trace::TraceKey;
use ac2_scene::view::{
    DistortionUnit, FreqRange, IrMode, IrPane, PlotChrome, SpectrumMode, SplMode, SweepMode,
    ViewState,
};
#[cfg(test)]
use ac2_scene::{axis::Range, view::PhaseView};

use crate::anim::FreqNav;
use crate::cal_view::{CalAction, CalView};
use crate::conn::{ConnEvent, DataSnapshot, Request, StimEvent};
use crate::forms::{Form, FormKind};
use crate::keys::{Chord, CommandId, Keymap, RESERVED, STOP_ANYWHERE, Scope};
use crate::leq_dialog::LeqDialog;
use crate::palette::Palette;
use crate::prefs::UiPrefs;
use crate::session_dialog::{RoleKey, Row, SessionDialog};
use crate::settings::{FocusWant, Page, Settings};

#[path = "state_display.rs"]
mod display;
pub use display::{ChoicePrompt, ChoicePurpose, DeletePrompt, DeleteTarget, MoveWhat};
#[path = "state_levels.rs"]
mod levels;
pub use levels::{LEVEL_ZOOM_FACTOR, PaneLevels};
#[path = "state_settings.rs"]
mod settings_impl;
pub use settings_impl::SettingsMsg;
#[path = "state_ir.rs"]
mod ir_nav;
pub use ir_nav::IrNavMsg;
#[path = "state_band_transfer.rs"]
mod band_transfer;
#[path = "state_commands.rs"]
mod commands;
#[path = "state_commands_meas.rs"]
mod commands_meas;
#[path = "state_commands_tf.rs"]
mod commands_tf;
#[path = "state_dialogs.rs"]
mod dialogs;
#[path = "state_drag.rs"]
mod drag;
pub use drag::{DragMsg, DropTarget, EDGE_SHARE, RowDrag, drop_target};
#[path = "state_glide.rs"]
mod glide;
#[path = "state_keys.rs"]
mod keys;
pub use glide::SpecGlide;
#[path = "state_leq.rs"]
mod leq;
#[path = "state_leq_strip.rs"]
mod leq_strip;
pub use leq_strip::{LeqSpan, STRIP_MIN_S, StripMsg};
#[path = "state_leq_export.rs"]
mod leq_export;
pub use leq_export::{EXPORT_DIR, export_path, leq_export_name};
#[path = "state_link.rs"]
mod link;
#[path = "state_meas_display.rs"]
mod meas_display;
#[path = "state_meas_layout.rs"]
mod meas_layout;
#[path = "state_new_meas.rs"]
mod new_meas;
#[path = "state_panes.rs"]
mod panes;
#[path = "state_panes_ops.rs"]
mod panes_ops;
#[path = "state_panes_views.rs"]
mod panes_views;
#[path = "state_queries.rs"]
mod queries;
#[path = "state_queries_inputs.rs"]
mod queries_inputs;
#[path = "state_queries_layout.rs"]
mod queries_layout;
#[path = "state_select.rs"]
mod select;
#[path = "state_sessions.rs"]
pub(crate) mod sessions_impl;
pub use sessions_impl::SessionsMsg;
#[path = "state_stimulus.rs"]
mod stimulus;
#[path = "state_text.rs"]
mod text;
#[path = "state_undo.rs"]
mod undo;
pub use panes::{Axis, PaneId, PaneNode, PaneRect, Side, SplitGap};
pub use panes_views::{
    DEFAULT_PANE_AREA, Layout, PaneMenuRow, PaneModes, PaneTemplate, TransferView, View,
};
pub use text::{
    curve_what, kind_shown, meas_input, mics_text, open_session_hint, parse_band, parse_mics,
    parse_number, parse_outputs, parse_session_ref, parse_slot, trace_label,
};
use text::{drawn_in, offset_text, slot_of};
pub use undo::HISTORY_LEN;

#[path = "state_types.rs"]
mod types;
pub(crate) use types::STIMULUS_STOPPED_TOO;
use types::trace_mic_request;
pub use types::{
    ConnState, FinderChoice, LiveEdit, MAX_OBSERVATION_S, PaneKind, PaneMenu, SMOOTHING_STEPS,
    SUB_OBSERVATIONS_S, SmoothTarget, Smoothable, StimPhase, Stimulus, outputs_text, parse_curve,
    parse_input_curve, spectrum_smoothing,
};
#[path = "state_msg.rs"]
mod msg;
pub use msg::{
    CAL_PAGE, COHERENCE_MASKS, DELAY_FINE_STEP, DelayChoice, FormMsg, HELP_LINE, HELP_PAGE,
    LEVEL_FLOOR, LegendMsg, LeqMsg, MAX_TOASTS, Msg, NUDGE_S, NewLogPrompt, Notice, Overlay,
    PAN_OCTAVES, PREVIEW_RENEW_S, Prompt, PromptKind, RECORD_MAX_S, SessionMsg, Toast, ZOOM_FACTOR,
};
use msg::{LeqLogSeen, Muted};

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
    /// Per Leq and band Leq topic, the frame the newest one replaced: the bars ease from it
    /// ([`ac2_scene::bar_ease`]).
    pub bar_from: HashMap<Topic, Arc<ac2_proto::Frame>>,
    /// Per live spectrum or RTA topic, the glide of its curve to the newest frame
    /// ([`ac2_scene::curve_ease`]).
    pub spec_glide: HashMap<Topic, SpecGlide>,
    /// Per live spectrum or RTA topic, the capture times and arrivals that pace its glides.
    pub spec_pacing: HashMap<Topic, ac2_scene::curve_ease::Pacing>,
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
    /// The size of the panes' area as last drawn (the view writes it): N splits the
    /// focused pane along its longer side.
    pub pane_area: (f32, f32),
    /// The stored trace selected (list, V): the trace keys change it
    /// instead of the pane's measurement, and a selected sweep is what the sweep pane shows.
    /// Selecting a measurement clears it, so whichever of the two was selected last is what
    /// the keys act on ([`AppState::keys_on_trace`]).
    pub selected_trace: Option<TraceId>,
    /// The measurement whose live-curve row was picked (Tab, its legend row or its list
    /// row) rather than its header. The live curve is what the measurement's keys act on
    /// either way; this only says which of the two rows is the selection, so Tab steps
    /// header → live curve → stored traces. Meaningful while it is the selected measurement
    /// and no stored trace has the keys ([`AppState::live_row_selected`]).
    pub selected_live: Option<MeasId>,
    /// Measurements whose live curves this app hides: display only, they keep measuring.
    /// By id: names may repeat, and hiding one `Main` must not hide another. `ui.toml`
    /// keeps them by name ([`AppState::restore_meas_choices`]).
    pub hidden_meas: BTreeSet<MeasId>,
    /// Groups of the measurement tree folded in this app (their rows not listed).
    pub collapsed: BTreeSet<ac2_proto::model::TraceOwner>,
    pub edits: BTreeMap<MeasId, LiveEdit>,
    /// Peak hold per spectrum / RTA measurement, with the last folded-in `seq` and capture
    /// time.
    pub peaks: BTreeMap<MeasId, (u64, u64, PeakHold)>,
    /// Tracked peaks per spectrum / RTA measurement, with the last folded-in `seq`, kept
    /// while peak tracking is on.
    pub peak_tracks: BTreeMap<MeasId, (u64, ac2_scene::spectrum::peaks::PeakTracker)>,
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
    /// This app's build as Settings › Connection shows it ([`crate::BUILD_ID`]; screenshot
    /// tests pin it, since it names the commit).
    pub build_id: String,
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
    /// What rasterises the window: a CPU rasteriser gets fewer frames and no shadows.
    pub rasterizer: ac2_plot::Rasterizer,
    /// The last [`ac2_scene::toast::LOG_LEN`] notifications, oldest first.
    pub notices: std::collections::VecDeque<Notice>,
    pub now_s: f64,
    pub quit: bool,
    /// The app hosts its daemon (`None`: it connects to one); `Some(true)`: that daemon
    /// autosaves. Quitting with unsaved changes then asks first.
    pub embedded_autosave: Option<bool>,
    /// The window fills the screen (the app applies it).
    pub fullscreen: bool,
    /// With `fullscreen`: the panes alone, every pane of the layout, without the top bar
    /// and the lists (F's second step).
    pub panes_only: bool,
    /// The key that just opened a text overlay also arrives as text; drop that one char.
    swallow_text: Option<char>,
    /// Settings sent with the arm in flight; a change made before the arm is confirmed is
    /// sent once it is.
    armed_with: Option<GeneratorSettings>,
    /// A measurement this client just created: selected once the mirror lists it.
    pending_select: Option<MeasId>,
    /// The name of a transfer measurement submitted from its form: its creation lays it
    /// out in the transfer stack.
    stack_on_create: Option<String>,
    /// That measurement, created: stacked once the mirror lists it.
    pending_stack: Option<MeasId>,
    /// The name of a sweep, spectrum, RTA or SPL meter submitted from its form: its
    /// creation brings up a pane of its kind for it.
    pane_on_create: Option<String>,
    /// Open the Sessions dialog once the daemon's state shows no audio session (an embedded
    /// daemon on real audio starts without one): its New leads on to the audio setup.
    pub open_session_when_empty: bool,
    /// The session dialog's session opened: ask what to measure ([`Self::choose_first`])
    /// once the daemon's state shows it open.
    choose_when_open: bool,
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
    /// The span (daemon wall clock, s) each SPL pane's history strip looks back at; a
    /// pane not here follows live.
    pub leq_strips: BTreeMap<PaneId, (f64, f64)>,
    /// The pointer over a history strip: its pane and time (s).
    pub leq_cursor: Option<(PaneId, f64)>,
    /// Spans of each meter's log fetched for strips looking back, oldest first.
    pub leq_spans: BTreeMap<MeasId, Vec<leq_strip::LeqSpan>>,
    /// The span fetch under way.
    leq_span_pending: Option<leq_strip::SpanAsk>,
    /// Number of the newest span fetch.
    leq_span_ask: u64,
    /// Width of the strip last moved by the mouse, px: the resolution fetched at.
    pub leq_strip_px: f32,
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
    /// Hidden measurements by name, waiting for the daemon to list one of that name: read
    /// from `ui.toml`, or a hidden one the daemon took away (a session reopening, a daemon
    /// restarting renumbers them).
    pending_hidden: BTreeSet<String>,
    /// Each pane's level axis ranges, by pane: a pane not here draws on the defaults.
    pane_levels: BTreeMap<PaneId, PaneLevels>,
    /// Spectrum panes whose level axis is framed for what they show, or set by the operator:
    /// a pane not here frames its next frame.
    spectrum_hold: BTreeMap<PaneId, levels::SpectrumHold>,
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
    /// A key moved the Settings page's focus (or a page was shown): the page brings the
    /// focused row into view once (the view takes it), so the wheel and the mirror's
    /// updates leave the scroll where the operator put it.
    pub settings_reveal: bool,
    /// The Settings page last shown: the Settings key opens it again.
    settings_page_last: Page,
    /// The SPL / Leq page's cue fold as the operator last set it, for the app's run: Apply,
    /// closing and another meter keep it (a meter with a cue set opens shown anyway).
    leq_cues_shown: bool,
    /// The SPL / Leq page last applied, with what its call is named: a conflict brings it
    /// back with its edits.
    leq_sent: Option<(String, Box<LeqDialog>)>,
    /// `meas.layout` calls not answered yet, oldest first.
    layout_calls: std::collections::VecDeque<meas_layout::LayoutCall>,
    /// Edits waiting for the `meas.layout` call in flight for their measurement: the newest.
    layout_queued: BTreeMap<MeasId, ac2_proto::model::PaneLayout>,
    /// A split dragged: the measurement whose layout takes it, and when (`now_s`) the drag
    /// counts as done.
    layout_due: Option<(MeasId, f64)>,
    /// A measurement's row was chosen: its own panes go on screen after the message.
    own_panes_due: bool,
    /// The measurement this app selected by itself, the one it had gone (or none chosen
    /// yet): a stand-in while it stays selected, whose own panes never come on screen.
    standin: Option<MeasId>,
    /// The selected measurement that went, by name, while a stand-in (or nothing) is
    /// selected: one of that name coming back is selected again.
    lost_selection: Option<String>,
    /// The measurement whose own panes the operator put on screen here by choosing its row.
    /// An edit of the panes is stored as its layout (while a pane still shows it), even
    /// after a pane or child row selected another measurement: that one is a guest in its
    /// panes. While it is selected, another client's edit of its layout shows here too; no
    /// other measurement's does: a viewer nobody touches keeps its panes.
    own_panes: Option<MeasId>,
    /// The lost selection's own panes were on screen: they follow it when it comes back.
    lost_own_panes: bool,
    /// The measurement whose row was chosen last (or that this app selected by itself, a
    /// measurement made, a sweep just run): a pane with no pick of its own shows it. A child
    /// row, a chip pick or a click in a pane never moves it, so they change no pane but the
    /// one picked.
    chosen: Option<MeasId>,
    /// The level ranges the operator set, by pane: each is stored with the panes and kept
    /// (never framed again automatically) until the pane shows another measurement.
    level_set: BTreeSet<(PaneId, levels::LevelSlot)>,
    /// Names of the measurements the daemon has listed since this app started: a pane
    /// remembered on one of them waits for it to come back.
    meas_seen: BTreeSet<String>,
    /// The keymap of the latest update: toasts and warnings name the keys it binds.
    keys: Keymap,
    /// Started spectrum / RTA measurements whose first frame fits the spectrum pane's level
    /// axis (as Shift+Home), with the frame shown when they started (a stopped run's last).
    spectrum_fit: BTreeMap<MeasId, Option<Arc<ac2_proto::Frame>>>,
    /// A list row being dragged onto the panes.
    row_drag: Option<RowDrag>,
    /// Edits of the panes that undo takes back.
    history: undo::History,
    /// The message is an undo or redo: it puts back panes, it edits none.
    undoing: bool,
    /// The first command the message ran (a key, the palette): what an edit was.
    ran_command: Option<CommandId>,
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
    /// The theme the panes are drawn with.
    pub fn scene_theme(&self) -> ac2_scene::theme::Theme {
        ac2_scene::theme::Theme::by_name(self.theme)
    }

    pub fn new(theme: ThemeName, target: String) -> Self {
        Self {
            conn: ConnState::Connecting { target },
            mirror: None,
            data: None,
            bar_from: HashMap::new(),
            spec_glide: HashMap::new(),
            spec_pacing: HashMap::new(),
            traces: BTreeMap::new(),
            theme,
            view: ViewState::default(),
            nav: FreqNav::new(FreqRange::default()),
            layout: Layout::default(),
            selected: None,
            pane_area: DEFAULT_PANE_AREA,
            selected_trace: None,
            selected_live: None,
            edits: BTreeMap::new(),
            peaks: BTreeMap::new(),
            peak_tracks: BTreeMap::new(),
            spectrographs: BTreeMap::new(),
            stimulus: Stimulus::default(),
            sweep: SweepUi::default(),
            prefs: UiPrefs::default(),
            prefs_dirty: false,
            export_dir: std::env::home_dir(),
            local_zone: crate::scenes::LocalZone::System,
            build_id: crate::BUILD_ID.into(),
            stim_device: None,
            finder: FinderChoice::default(),
            overlay: Overlay::None,
            help_scroll: 0.0,
            help_page: HELP_PAGE,
            toasts: Vec::new(),
            toasts_held: false,
            toast_seq: 0,
            rasterizer: ac2_plot::Rasterizer::Gpu,
            notices: std::collections::VecDeque::new(),
            now_s: 0.0,
            quit: false,
            embedded_autosave: None,
            fullscreen: false,
            panes_only: false,
            swallow_text: None,
            armed_with: None,
            pending_select: None,
            stack_on_create: None,
            pane_on_create: None,
            pending_stack: None,
            open_session_when_empty: false,
            choose_when_open: false,
            devices: None,
            preview_sent_s: f64::NEG_INFINITY,
            devices_for: None,
            leq_history: BTreeMap::new(),
            leq_strips: BTreeMap::new(),
            leq_cursor: None,
            leq_spans: BTreeMap::new(),
            leq_span_pending: None,
            leq_span_ask: 0,
            leq_strip_px: 600.0,
            tf_display: Default::default(),
            leq_logs: BTreeMap::new(),
            leq_backfill_ask: 0,
            leq_alarms_seen: BTreeMap::new(),
            spl_hold: BTreeMap::new(),
            pending_pane_meas: BTreeMap::new(),
            pane_auto: true,
            pending_hidden: BTreeSet::new(),
            pane_levels: BTreeMap::new(),
            spectrum_hold: BTreeMap::new(),
            hidden_meas: BTreeSet::new(),
            collapsed: BTreeSet::new(),
            link_wants: crate::link_wants::Sent::default(),
            spectrum_running: BTreeSet::new(),
            spectrum_fit: BTreeMap::new(),
            client_key: None,
            want_connect_dialog: false,
            tree_reveal: false,
            settings_reveal: false,
            settings_page_last: Page::AudioIo,
            leq_cues_shown: false,
            leq_sent: None,
            layout_calls: std::collections::VecDeque::new(),
            layout_queued: BTreeMap::new(),
            layout_due: None,
            own_panes_due: false,
            standin: None,
            lost_selection: None,
            own_panes: None,
            lost_own_panes: false,
            chosen: None,
            level_set: BTreeSet::new(),
            meas_seen: BTreeSet::new(),
            keys: Keymap::default(),
            row_drag: None,
            history: undo::History::default(),
            undoing: false,
            ran_command: None,
        }
    }

    // ----- queries -----------------------------------------------------------------------

    pub fn update(&mut self, msg: Msg, keymap: &Keymap) -> Vec<Request> {
        let mut out = Vec::new();
        let before = self.meter_wants();
        let tick = matches!(msg, Msg::Tick { .. });
        let drag = matches!(msg, Msg::DragSplit { .. });
        let conn = matches!(msg, Msg::Conn(_));
        if !tick && !drag {
            self.flush_layout_drag(&mut out);
            self.finish_split_drag();
        }
        if self.keys != *keymap {
            self.keys = keymap.clone();
        }
        let panes_before = self.panes_watch();
        let edit = if drag {
            self.start_split_drag();
            None
        } else if tick || conn {
            None
        } else {
            self.undo_watch(&msg)
        };
        self.ran_command = None;
        self.undoing = false;
        self.update_inner(msg, keymap, &mut out);
        if let Some(c) = self.overlay.leq().and_then(|d| d.cues_chosen) {
            self.leq_cues_shown = c;
        }
        if tick {
            self.poll_settings(&mut out);
            self.layout_drag_due(&mut out);
        }
        self.sync_meters(before, tick, &mut out);
        self.sync_session_watch(&mut out);
        if !tick {
            self.restore_pane_meas();
            self.restore_meas_choices();
            self.follow_standin();
            // An undo or redo stores what it restores itself, on the owner of the edit only.
            let undoing = std::mem::take(&mut self.undoing);
            self.sync_meas_layout(panes_before, conn || undoing, drag, &mut out);
            if let Some(w) = edit.filter(|_| !undoing) {
                let ran = self.ran_command;
                self.record_edit(w, ran);
            }
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
            Msg::Conn(e) => self.conn_event(*e, out),
            Msg::Form(m) => self.form_msg(m, out),
            Msg::Session(m) => self.session_msg(m, out),
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
            Msg::LeqStrip(p, m) => self.leq_strip_msg(p, m, out),
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
            Msg::Rasterizer(r) => {
                if r == ac2_plot::Rasterizer::Software && self.rasterizer != r {
                    self.toast(ac2_scene::toast::SOFTWARE_RENDERING);
                }
                self.rasterizer = r;
            }
            Msg::DismissToast(id) => {
                self.toasts.retain(|t| t.id != id);
                // What moved under the pointer is not what it rested on.
                self.toasts_held = false;
            }
            Msg::SelectMeas(id) => self.select_meas_row(id),
            Msg::OpenRig => self.open_settings(crate::settings::Page::Rig, out),
            Msg::SelectLive(id) => self.select_live_row(id),
            Msg::SelectMath(id) => self.select_math_row(id),
            Msg::SelectTrace(id) => {
                if self.selected_trace == Some(id) {
                    self.set_selected_trace(None);
                } else {
                    self.select_trace_row(id);
                }
            }
            Msg::ToggleShown(id) => self.toggle_shown(id, out),
            Msg::ToggleGroup(g) => {
                if !self.collapsed.remove(&g) {
                    self.collapsed.insert(g);
                }
            }
            Msg::ToggleMeasShown(id) => self.toggle_meas_hidden(id, None),
            Msg::Choose(pick) => self.choose(pick, out),
            Msg::Sessions(m) => self.sessions_msg(m, out),
            Msg::RenameTrace(id) => {
                if self.selected_trace != Some(id) {
                    self.select_trace_row(id);
                }
                if let Ok(t) = self.trace_meta(id) {
                    self.prompt(PromptKind::TraceRename(id), t.edit.name.clone());
                }
            }
            // A click in a pane gives it the keyboard; the selection stays on the row the
            // operator chose (the pane's keys act on what it shows).
            Msg::FocusPane(p) => self.layout.set_focus(p),
            Msg::LegendPick(p, key) => self.legend_pick(p, key),
            Msg::PaneMenu(p) => {
                self.overlay = match self.overlay {
                    Overlay::PaneMenu(m) if m.pane == p => Overlay::None,
                    _ => self.pane_menu(p),
                };
            }
            Msg::PanePick(p, row) => self.pane_pick(p, row),
            Msg::DragSplit { path, ratio } => self.drag_split(&path, ratio),
            // The drag's step was closed before the message, as any other message closes it.
            Msg::DragSplitEnd => {}
            Msg::RowDrag(m) => self.row_drag_msg(m, out),
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
