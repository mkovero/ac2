//! Messages, overlays, prompts and notices the reducer handles.

use super::*;

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
    /// Where an SPL meter's current (or previous) Leq log is written as CSV on this
    /// computer.
    LeqExport {
        meas: MeasId,
        previous: bool,
    },
    /// Where a pane is written as PNG on this computer.
    PaneExport(PaneId),
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
                "Export the selected trace to (file path: .wav writes its impulse response, \
                 anything else ac2 CSV; a folder takes the trace's name)"
            }
            PromptKind::Delay(_) => "Delay (ms)",
            PromptKind::ImportFile(ImportRole::Target) => "Target curve file (path)",
            PromptKind::ImportFile(ImportRole::Trace) => {
                "Trace file to import: CSV, analyzer text or a WAV impulse response (path; \
                 a WAV asks its channel, time zero and gate next)"
            }
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
            PromptKind::LeqExport {
                previous: false, ..
            } => "Export the Leq log as CSV on this computer to (file path)",
            PromptKind::LeqExport { previous: true, .. } => {
                "Export the previous Leq log as CSV on this computer to (file path)"
            }
            PromptKind::PaneExport(_) => {
                "Export the focused pane as PNG on this computer to (file path)"
            }
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
    /// The confirmation before a new SPL log.
    NewLog(Box<NewLogPrompt>),
    /// The confirmation before the selected measurement or stored trace is deleted (or,
    /// for a measurement a math channel computes from, why it cannot be).
    Delete(Box<DeletePrompt>),
    /// A question with a few answers (←/→ or ↑/↓ pick, Enter takes it, Esc cancels):
    /// what deleting a measurement does with its traces, where a trace moves, unsaved
    /// changes before a session is replaced, the kind of a new measurement.
    Choose(Box<ChoicePrompt>),
    /// The Sessions dialog (Alt+S): new, open, save, recents, the file browser.
    Sessions(Box<crate::sessions::SessionsDialog>),
}

impl Overlay {
    /// A window to read that leaves the keys working (except the stimulus's and those
    /// that scroll it), with nothing typed into it.
    pub fn is_reading(&self) -> bool {
        matches!(self, Overlay::Help | Overlay::Notifications)
    }

    /// A window that takes typed text: every text event goes to it, never to a shortcut
    /// or a widget behind it. The Sessions dialog types only on its name page; its lists
    /// keep a held Backspace from climbing several folders.
    pub fn takes_text(&self) -> bool {
        match self {
            Overlay::Palette(_) | Overlay::Prompt(_) | Overlay::Form(_) | Overlay::Settings(_) => {
                true
            }
            Overlay::Sessions(d) => matches!(d.page, crate::sessions::Page::Name(_)),
            _ => false,
        }
    }

    /// The Settings view, when open.
    pub fn settings(&self) -> Option<&Settings> {
        match self {
            Overlay::Settings(s) => Some(s),
            _ => None,
        }
    }

    /// The session model while Settings shows the Audio I/O page.
    pub fn session(&self) -> Option<&SessionDialog> {
        match self {
            Overlay::Settings(s) if matches!(s.page, Page::AudioIo) => Some(&s.session),
            _ => None,
        }
    }

    pub fn session_mut(&mut self) -> Option<&mut SessionDialog> {
        match self {
            Overlay::Settings(s) if matches!(s.page, Page::AudioIo) => Some(&mut s.session),
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
pub(super) struct LeqLogSeen {
    /// The windows it was rebuilt for.
    pub(super) config: ac2_proto::model::LeqConfig,
    /// Rows logged as of the newest `leq` frame: a new log numbers its rows from 0 again.
    pub(super) logged: u64,
    /// The log held a second (the `spl_log` entity's `started_at`): `spl.log_new` empties it.
    pub(super) started: bool,
    /// The rebuild asked for; an answer to an earlier one is stale.
    pub(super) ask: u64,
}

/// The confirmation before `spl.log_new`: which meter, and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct NewLogPrompt {
    pub meas: MeasId,
    pub meter: String,
    pub confirm: ac2_scene::leq::NewLogConfirm,
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
pub(super) enum Muted {
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
    /// What rasterises the window, known once its adapter is chosen.
    Rasterizer(ac2_plot::Rasterizer),
    /// A measurement's row clicked in the list: selected, and its own panes on screen (or
    /// the focused pane shows it).
    SelectMeas(MeasId),
    /// The rig group's header in the list: the Rig monitor settings.
    OpenRig,
    /// A measurement's live-curve row clicked in the list: a child row, shown by the
    /// focused pane alone, the live row marked as what the keys act on.
    SelectLive(MeasId),
    /// A math channel's row clicked in the list: a child row, shown by the focused pane
    /// alone.
    SelectMath(MeasId),
    /// A stored trace clicked in the list: selected, so the trace keys act on it, and its
    /// measurement shown by the focused pane alone (again: deselected).
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
    /// The gap of the split at `path` dragged: the split takes `ratio`, within its limits.
    DragSplit {
        path: Vec<Side>,
        ratio: f32,
    },
    /// A split's gap released: the drag is one step of the undo history.
    DragSplitEnd,
    /// A row of the measurement list dragged onto the panes.
    RowDrag(super::DragMsg),
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
    /// A click on the row of pane `p`'s legend that names `key`'s curve.
    LegendPick(PaneId, ac2_scene::trace::TraceKey),
    /// Mouse on an open dialog.
    Form(FormMsg),
    /// Mouse on the session dialog.
    Session(SessionMsg),
    /// Mouse on the new SPL log confirmation: `true` starts it, `false` keeps the log.
    NewLog(bool),
    /// Mouse on the delete confirmation: `true` deletes, `false` keeps it.
    Delete(bool),
    /// Mouse on a question's answers: one taken, or `None` (cancel).
    Choose(Option<usize>),
    /// Mouse or touch on the Sessions dialog.
    Sessions(sessions_impl::SessionsMsg),
    /// A group's arrow in the measurement tree: folds or unfolds it.
    ToggleGroup(ac2_proto::model::TraceOwner),
    /// A live curve's dot in the tree: shown / hidden (this app's display).
    ToggleMeasShown(MeasId),
    /// Ctrl+wheel on a pane: its level axis zooms by `factor` (> 1 in) about `about_db`.
    LevelZoom {
        pane: PaneId,
        about_db: Option<f64>,
        factor: f64,
    },
    /// Shift+wheel on a pane: its level axis pans by `db` (positive: higher levels).
    LevelPan {
        pane: PaneId,
        db: f64,
    },
    /// Mouse on the Leq windows dialog.
    Leq(LeqMsg),
    /// Mouse on the Settings view.
    Settings(SettingsMsg),
    /// Mouse on an impulse-response picture (the transfer and sweep panes' IR views).
    IrNav(ac2_scene::view::IrPane, IrNavMsg),
    /// The mouse on an SPL pane's Leq history strip.
    LeqStrip(PaneId, StripMsg),
}

/// What the mouse does on the Leq windows dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeqMsg {
    Focus(crate::leq_dialog::Focus),
    /// A click on a cue line: focused, the caret before its character `usize`.
    Caret(crate::leq_dialog::Focus, usize),
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
    /// The chosen device's driver settings: an ASIO control panel, Audio MIDI Setup.
    Panel,
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
