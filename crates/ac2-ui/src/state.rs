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
    FinderBand, GeneratorDesired, GeneratorSettings, ImportRole, InputKind, InputSetup, MathDomain,
    MeasKind, Measurement, MicCurveId, Operand, Polarity, SessionRef, Signal, Smoothing,
    SmoothingFraction, SmoothingMode, State, SweepStatus, TraceData, TraceKind, TraceMeta,
    TraceOwner,
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

mod display;
pub use display::{ChoicePrompt, ChoicePurpose, DeletePrompt, DeleteTarget, MoveWhat};
pub use panes::levels::{LEVEL_ZOOM_FACTOR, PaneLevels};
mod settings_impl;
pub use settings_impl::SettingsMsg;
mod ir_nav;
pub use ir_nav::IrNavMsg;
mod band_transfer;
mod commands;
pub use panes::drag::{DragMsg, DropTarget, EDGE_SHARE, RowDrag, drop_target};
mod advice;
mod glide;
pub use glide::SpecGlide;
mod leq;
pub use leq::LeqUi;
pub use leq::export::{EXPORT_DIR, export_path, leq_export_name};
pub use leq::strip::{LeqSpan, STRIP_MIN_S, StripMsg};
pub use panes::pane_export::{PaneExport, pane_export_name};
mod link;
use link::Pending;
use panes::meas_layout::LayoutQueue;
mod drop_math;
mod programme;
pub use programme::{MAX_METERED, Metered, ProgrammeUi};
mod meter_form;
mod new_meas;
mod panes;
mod queries;
pub(crate) mod sessions_impl;
mod target;
pub use drop_math::DropPlan;
pub use sessions_impl::SessionsMsg;
mod eq_impl;
mod stimulus;
mod text;
mod undo;
pub use panes::views::{
    DEFAULT_PANE_AREA, Layout, PaneMenuRow, PaneModes, PaneTemplate, TransferView, View,
};
pub use panes::{Axis, PaneId, PaneNode, PaneRect, Side, SplitGap};
pub use text::{
    curve_what, kind_shown, meas_input, mics_text, open_session_hint, parse_band, parse_mics,
    parse_session_ref, parse_slot, trace_label,
};
use text::{drawn_in, offset_text, slot_of};
pub use undo::HISTORY_LEN;

mod types;
pub(crate) use types::STIMULUS_STOPPED_TOO;
use types::trace_mic_request;
pub use types::{
    ConnState, FinderChoice, LiveEdit, MAX_OBSERVATION_S, PaneKind, PaneMenu, SMOOTHING_STEPS,
    SUB_OBSERVATIONS_S, SmoothTarget, Smoothable, StimPhase, Stimulus, outputs_text, parse_curve,
    parse_input_curve, spectrum_smoothing,
};
mod msg;
pub use msg::{
    CAL_PAGE, COHERENCE_MASKS, DELAY_FINE_STEP, DelayChoice, FormMsg, HELP_LINE, HELP_PAGE,
    LEVEL_FLOOR, LeqMsg, MAX_TOASTS, Msg, NUDGE_S, NewLogPrompt, Notice, Overlay, PAN_OCTAVES,
    PREVIEW_RENEW_S, PlateMsg, Prompt, PromptKind, RECORD_MAX_S, SessionMsg, Toast, Toasts,
    ZOOM_FACTOR,
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
    /// Per running transfer measurement, its headroom advice over time
    /// ([`ac2_scene::advice::Hold`]).
    pub advice: HashMap<MeasId, ac2_scene::advice::Hold>,
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
    /// Shown notifications.
    pub toasts: Toasts,
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
    /// What waits for the mirror to list it.
    pending: Pending,
    /// The name of a transfer measurement submitted from its form: its creation lays it
    /// out in the transfer stack.
    stack_on_create: Option<String>,
    /// The name of a sweep, spectrum, RTA or SPL meter submitted from its form: its
    /// creation brings up a pane of its kind for it.
    pane_on_create: Option<String>,
    /// The name of an alignment submitted from the math dialog: its creation splits the
    /// focused pane and the new half shows its alignment view.
    align_on_create: Option<String>,
    /// Open the Sessions dialog once the daemon's state shows no audio I/O (an embedded
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
    /// The SPL meters' Leq history, strips, logs and alarms.
    pub leq: LeqUi,
    /// Each SPL meter's displayed reading, held for its display period.
    pub spl_hold: BTreeMap<MeasId, ac2_scene::spl::SplHold>,
    /// The Programme panes' flying peaks and ember rings.
    pub programme: ProgrammeUi,
    /// The Inputs-list row picked (its input, a pair's left): the keys go to its meters
    /// until a pane or another row is clicked.
    pub input_pick: Option<u16>,
    /// Each pane's level axis ranges, by pane: a pane not here draws on the defaults.
    pane_levels: BTreeMap<PaneId, PaneLevels>,
    /// Spectrum panes whose level axis is framed for what they show, or set by the operator:
    /// a pane not here frames its next frame.
    spectrum_hold: BTreeMap<PaneId, panes::levels::SpectrumHold>,
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
    /// A pane to write as PNG (the app renders it with the window's GPU and takes it).
    pub pane_export: Option<PaneExport>,
    /// The selection moved by key: the tree brings its row into view (the view takes it).
    pub tree_reveal: bool,
    /// A key moved the Settings page's focus (or a page was shown): the page brings the
    /// focused row into view once (the view takes it), so the wheel and the mirror's
    /// updates leave the scroll where the operator put it.
    pub settings_reveal: bool,
    /// The Settings page last shown: the Settings key opens it again.
    settings_page_last: Page,
    /// `meas.layout` calls in flight and the edits waiting for them.
    layout_queue: LayoutQueue,
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
    /// The plate under the pointer, and whose pane it is on.
    pub plate_hover: Option<(PaneId, ac2_scene::plate::PlateHover)>,
    /// The measurement whose row was chosen last (or that this app selected by itself, a
    /// measurement made, a sweep just run): a pane with no pick of its own shows it. A child
    /// row, a chip pick or a click in a pane never moves it, so they change no pane but the
    /// one picked.
    chosen: Option<MeasId>,
    /// The level ranges the operator set, by pane: each is stored with the panes and kept
    /// (never framed again automatically) until the pane shows another measurement.
    level_set: BTreeSet<(PaneId, panes::levels::LevelSlot)>,
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
            advice: HashMap::new(),
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
            toasts: Toasts::default(),
            rasterizer: ac2_plot::Rasterizer::Gpu,
            notices: std::collections::VecDeque::new(),
            now_s: 0.0,
            quit: false,
            embedded_autosave: None,
            fullscreen: false,
            panes_only: false,
            swallow_text: None,
            armed_with: None,
            pending: Pending::default(),
            stack_on_create: None,
            pane_on_create: None,
            align_on_create: None,
            open_session_when_empty: false,
            choose_when_open: false,
            devices: None,
            preview_sent_s: f64::NEG_INFINITY,
            devices_for: None,
            leq: LeqUi {
                history: BTreeMap::new(),
                strips: BTreeMap::new(),
                cursor: None,
                spans: BTreeMap::new(),
                span_pending: None,
                span_ask: 0,
                strip_px: 600.0,
                logs: BTreeMap::new(),
                backfill_ask: 0,
                alarms_seen: BTreeMap::new(),
                cues_shown: false,
                sent: None,
            },
            tf_display: Default::default(),
            spl_hold: BTreeMap::new(),
            programme: ProgrammeUi::default(),
            input_pick: None,
            pane_auto: true,
            pane_levels: BTreeMap::new(),
            spectrum_hold: BTreeMap::new(),
            hidden_meas: BTreeSet::new(),
            collapsed: BTreeSet::new(),
            link_wants: crate::link_wants::Sent::default(),
            spectrum_running: BTreeSet::new(),
            spectrum_fit: BTreeMap::new(),
            client_key: None,
            want_connect_dialog: false,
            pane_export: None,
            tree_reveal: false,
            settings_reveal: false,
            settings_page_last: Page::AudioIo,
            layout_queue: LayoutQueue::default(),
            own_panes_due: false,
            standin: None,
            lost_selection: None,
            own_panes: None,
            lost_own_panes: false,
            plate_hover: None,
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
        // A split or a plate dragged: one undo step, sent to the daemon once it rests.
        let drag = matches!(
            msg,
            Msg::DragSplit { .. }
                | Msg::Plate(
                    PlateMsg::Move { .. } | PlateMsg::Scale { .. } | PlateMsg::Reset { .. }
                )
        );
        let conn = matches!(msg, Msg::Conn(_));
        // A click anywhere else in the list or the panes gives the keys back to the panes.
        if matches!(
            msg,
            Msg::FocusPane(_)
                | Msg::SelectMeas(_)
                | Msg::SelectLive(_)
                | Msg::SelectMath(_)
                | Msg::SelectTrace(_)
                | Msg::PaneMenu(_)
                | Msg::PanePick(..)
                | Msg::OpenRig
        ) {
            self.input_pick = None;
        }
        if !tick && !drag {
            self.flush_layout_drag(&mut out);
            self.finish_drag();
        }
        if self.keys != *keymap {
            self.keys = keymap.clone();
        }
        let panes_before = self.panes_watch();
        let edit = if drag {
            self.start_drag(&msg);
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
            self.leq.cues_shown = c;
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
            Msg::Text(t) => self.text_with(&t, out),
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
                if self.toasts.held {
                    for t in &mut self.toasts.list {
                        t.until_s += held;
                    }
                }
                let now = self.now_s;
                self.toasts.list.retain(|t| t.until_s > now);
                self.retire_embers();
            }
            Msg::ToastsHeld(held) => self.toasts.held = held,
            Msg::Rasterizer(r) => {
                if r == ac2_plot::Rasterizer::Software && self.rasterizer != r {
                    self.toast(ac2_scene::toast::SOFTWARE_RENDERING);
                }
                self.rasterizer = r;
            }
            Msg::DismissToast(id) => {
                self.toasts.list.retain(|t| t.id != id);
                // What moved under the pointer is not what it rested on.
                self.toasts.held = false;
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
                // An EQ set's curve is named by its set: a double click opens the set.
                if let Ok(ac2_proto::model::TraceSource::Eq { meas, set, .. }) =
                    self.trace_meta(id).map(|t| t.source)
                {
                    self.open_eq_on(meas, set);
                } else if let Ok(t) = self.trace_meta(id) {
                    self.prompt(PromptKind::TraceRename(id), t.edit.name.clone());
                }
            }
            // A click in a pane gives it the keyboard; the selection stays on the row the
            // operator chose (the pane's keys act on what it shows).
            Msg::FocusPane(p) => self.layout.set_focus(p),
            Msg::InputPick(c) => {
                self.input_pick = (self.input_pick != Some(c)).then_some(c);
            }
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
            Msg::Plate(m) => self.plate_msg(m),
            Msg::LegendScroll { first } => self.view.tf.legend.first = first,
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
                .list
                .retain(|t| !(t.severity == severity && t.text == text));
            if self.toasts.list.len() >= MAX_TOASTS {
                self.toasts.list.remove(0);
            }
            self.toasts.seq += 1;
            self.toasts.list.push(Toast {
                id: self.toasts.seq,
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

/// `press Shift+O (or Ctrl+K → Open audio I/O)`, from the keys actually bound.
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
mod tests;
