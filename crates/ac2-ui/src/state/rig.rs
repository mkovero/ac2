//! What the app holds for one daemon: its link, its mirror and data, and every choice keyed
//! by its ids.

use super::*;

/// The id of a rig whose daemon has not said hello yet. No daemon has it: `ac2d` makes its
/// id at random and never 0.
pub const UNKNOWN_RIG: RigId = RigId(0);

/// One daemon (a rig) as this app holds it. A daemon numbers its measurements, traces,
/// inputs and topics on its own, so two rigs' ids collide: everything keyed by them lives
/// here, never beside another rig's.
#[derive(Debug)]
pub struct RigState {
    /// Which rig this is in this run, whatever its id: a rig's id is the daemon's, known
    /// only once it says hello, and the slot names the rig before and after.
    pub slot: u32,
    /// The daemon's identity from its hello reply; [`UNKNOWN_RIG`] until the first one.
    pub id: RigId,
    /// The daemon's display name from its hello reply (empty until the first one).
    pub name: String,
    /// The link to the daemon (`None`: no link, as in the reducer tests). The reducer never
    /// touches it: the app sends this rig's requests over it and drains its events.
    pub link: Option<crate::conn::Conn>,
    /// Where the app reached this rig, remembered with it (`None`: not to be remembered, as
    /// a fake daemon, which is another rig each run).
    pub addr: Option<crate::prefs::RigAddr>,
    /// The link reaches a daemon inside this process: no network between, the same host.
    pub link_embedded: bool,
    /// The host the link reaches the daemon at over the network; `None` on a local link.
    pub daemon_addr: Option<String>,
    /// The daemon said hello as a rig this app already holds: the app closes this link.
    pub duplicate: bool,
    /// The transfer panes' phase reference while another rig is shown: it names this rig's
    /// traces.
    pub(super) phase_reference: Option<TraceKey>,
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
    /// Per transfer topic, its walk readiness bar's glide to the newest frame's `filled`
    /// ([`FillGlide`]).
    pub fill_glide: HashMap<Topic, FillGlide>,
    /// Per running transfer measurement, its headroom advice over time
    /// ([`ac2_scene::advice::Hold`]).
    pub advice: HashMap<MeasId, ac2_scene::advice::Hold>,
    /// Stored traces' data, as fetched.
    pub traces: BTreeMap<TraceId, (Arc<TraceData>, Arc<GridDef>)>,
    /// The transfer view's stored-trace display math, kept between frames.
    pub tf_display: ac2_scene::trace::DisplayCache,
    pub selected: Option<MeasId>,
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
    /// The output device the stimulus outputs belong to (the open session's).
    pub(super) stim_device: Option<String>,
    /// Band and observation X / Shift+X run the finder with.
    pub finder: FinderChoice,
    /// The app hosts its daemon (`None`: it connects to one); `Some(true)`: that daemon
    /// autosaves. Quitting with unsaved changes then asks first.
    pub embedded_autosave: Option<bool>,
    /// Settings sent with the arm in flight; a change made before the arm is confirmed is
    /// sent once it is.
    pub(super) armed_with: Option<GeneratorSettings>,
    /// What waits for the mirror to list it.
    pub(super) pending: Pending,
    /// The name of a transfer measurement submitted from its form: its creation lays it
    /// out in the transfer stack.
    pub(super) stack_on_create: Option<String>,
    /// The name of a sweep, spectrum, RTA or SPL meter submitted from its form: its
    /// creation brings up a pane of its kind for it.
    pub(super) pane_on_create: Option<String>,
    /// The name of an alignment (or a spatial average) submitted from the math dialog, and
    /// the view it opens in: its creation splits the focused pane and the new half shows it.
    pub(super) split_on_create: Option<(String, TransferView)>,
    /// Open the Sessions dialog once the daemon's state shows no audio I/O (an embedded
    /// daemon on real audio starts without one): its New leads on to the audio setup.
    pub open_session_when_empty: bool,
    /// The session dialog's session opened: ask what to measure ([`Self::choose_first`])
    /// once the daemon's state shows it open.
    pub(super) choose_when_open: bool,
    /// The daemon's backends and devices as last listed (channel names for the dialogs).
    pub devices: Option<Vec<ac2_proto::model::BackendInfo>>,
    /// When the device preview was last asked for.
    pub(super) preview_sent_s: f64,
    /// The session the device list was last asked for on its own (for the input names of
    /// the always-on meters).
    pub(super) devices_for: Option<ac2_proto::units::SessionEpoch>,
    /// The SPL meters' Leq history, strips, logs and alarms.
    pub leq: LeqUi,
    /// Each SPL meter's displayed reading, held for its display period.
    pub spl_hold: BTreeMap<MeasId, ac2_scene::spl::SplHold>,
    /// The Programme panes' flying peaks and ember rings.
    pub programme: ProgrammeUi,
    /// The Inputs-list row picked (its input, a pair's left): the keys go to its meters
    /// until a pane or another row is clicked.
    pub input_pick: Option<u16>,
    /// What the link was last asked to receive, and how often ([`crate::link_wants`]).
    pub(crate) link_wants: crate::link_wants::Sent,
    /// Whether the link was last asked for the input meters (a new link has none): each
    /// rig's link follows its own session, so it is asked by what it was asked last, never
    /// by what the shown rig wanted a moment ago.
    pub(crate) meters_asked: bool,
    /// Spectrum / RTA measurements seen running, so a start is told from a run going on.
    pub(super) spectrum_running: BTreeSet<MeasId>,
    /// `meas.layout` calls in flight and the edits waiting for them.
    pub(super) layout_queue: LayoutQueue,
    /// A measurement's row was chosen: its own panes go on screen after the message.
    pub(super) own_panes_due: bool,
    /// The measurement this app selected by itself, the one it had gone (or none chosen
    /// yet): a stand-in while it stays selected, whose own panes never come on screen.
    pub(super) standin: Option<MeasId>,
    /// The selected measurement that went, by name, while a stand-in (or nothing) is
    /// selected: one of that name coming back is selected again.
    pub(super) lost_selection: Option<String>,
    /// The measurement whose own panes the operator put on screen here by choosing its row.
    /// An edit of the panes is stored as its layout (while a pane still shows it), even
    /// after a pane or child row selected another measurement: that one is a guest in its
    /// panes. While it is selected, another client's edit of its layout shows here too; no
    /// other measurement's does: a viewer nobody touches keeps its panes.
    pub(super) own_panes: Option<MeasId>,
    /// The lost selection's own panes were on screen: they follow it when it comes back.
    pub(super) lost_own_panes: bool,
    /// The measurement whose row was chosen last (or that this app selected by itself, a
    /// measurement made, a sweep just run): a pane with no pick of its own shows it. A child
    /// row, a chip pick or a click in a pane never moves it, so they change no pane but the
    /// one picked.
    pub(super) chosen: Option<MeasId>,
    /// Names of the measurements the daemon has listed since this app started: a pane
    /// remembered on one of them waits for it to come back.
    pub(super) meas_seen: BTreeSet<String>,
    /// Started spectrum / RTA measurements whose first frame fits the spectrum pane's level
    /// axis (as Shift+Home), with the frame shown when they started (a stopped run's last).
    pub(super) spectrum_fit: BTreeMap<MeasId, Option<Arc<ac2_proto::Frame>>>,
}

/// What folding a rig's frames takes from the app: the operator's settings, the time, and
/// what the panes on screen show of that rig.
#[derive(Clone, Debug)]
pub(super) struct Folding {
    pub now_s: f64,
    pub spl_hold_ms: Option<u32>,
    pub spec_ease: bool,
    pub advice: ac2_scene::advice::AdviceSettings,
    pub cooling: ac2_scene::gonio::Cooling,
    /// The pairs a visible Programme pane shows: only those keep embers.
    pub pairs_shown: Vec<(u16, u16)>,
    /// A spectrum pane shows a spectrograph: only then are histories kept.
    pub spectrograph_shown: bool,
}

impl RigState {
    /// A rig not yet connected, reached through `target`.
    pub fn new(target: String) -> Self {
        Self::in_slot(0, target)
    }

    /// A rig not yet connected in slot `slot`: until its hello its id is the slot's own
    /// (the first rig's [`UNKNOWN_RIG`]), so no two rigs share one.
    pub fn in_slot(slot: u32, target: String) -> Self {
        Self {
            slot,
            id: RigId(u64::from(slot)),
            name: String::new(),
            link: None,
            addr: None,
            link_embedded: false,
            daemon_addr: None,
            duplicate: false,
            phase_reference: None,
            conn: ConnState::Connecting { target },
            mirror: None,
            data: None,
            bar_from: HashMap::new(),
            spec_glide: HashMap::new(),
            spec_pacing: HashMap::new(),
            fill_glide: HashMap::new(),
            advice: HashMap::new(),
            traces: BTreeMap::new(),
            tf_display: Default::default(),
            selected: None,
            selected_trace: None,
            selected_live: None,
            hidden_meas: BTreeSet::new(),
            collapsed: BTreeSet::new(),
            edits: BTreeMap::new(),
            peaks: BTreeMap::new(),
            peak_tracks: BTreeMap::new(),
            spectrographs: BTreeMap::new(),
            stimulus: Stimulus::default(),
            sweep: SweepUi::default(),
            stim_device: None,
            finder: FinderChoice::default(),
            embedded_autosave: None,
            armed_with: None,
            pending: Pending::default(),
            stack_on_create: None,
            pane_on_create: None,
            split_on_create: None,
            open_session_when_empty: false,
            choose_when_open: false,
            devices: None,
            preview_sent_s: f64::NEG_INFINITY,
            devices_for: None,
            leq: LeqUi::default(),
            spl_hold: BTreeMap::new(),
            programme: ProgrammeUi::default(),
            input_pick: None,
            link_wants: crate::link_wants::Sent::default(),
            meters_asked: false,
            spectrum_running: BTreeSet::new(),
            layout_queue: LayoutQueue::default(),
            own_panes_due: false,
            standin: None,
            lost_selection: None,
            own_panes: None,
            lost_own_panes: false,
            chosen: None,
            meas_seen: BTreeSet::new(),
            spectrum_fit: BTreeMap::new(),
        }
    }

    /// Folds a new data snapshot into what is kept of it (peaks, spectrographs, Leq
    /// histories, held readings, glides, advice, programme rings) and keeps it as the newest.
    /// Returns the meters with a new log, as [`Self::follow_leq_logs`].
    #[must_use]
    pub(super) fn fold_data(
        &mut self,
        d: Arc<DataSnapshot>,
        fold: &Folding,
        out: &mut Vec<Request>,
    ) -> Vec<MeasId> {
        self.fold_peaks(&d);
        self.fold_peak_tracks(&d);
        if fold.spectrograph_shown {
            self.fold_spectrographs(&d);
        }
        // A new log clears the history before its first frame goes in.
        let forgot = self.follow_leq_logs(Some(&d), out);
        self.fold_leq(&d);
        self.fold_spl(&d, fold);
        self.keep_bar_from(&d);
        self.keep_spec_glide(&d, fold.spec_ease);
        self.keep_fill_glide(&d);
        self.fold_advice(&d, fold);
        self.fold_programme(&d, fold);
        self.data = Some(d);
        forgot
    }

    /// The mirrored daemon state.
    pub fn daemon(&self) -> Option<&State> {
        self.mirror.as_ref().and_then(|m| m.state.as_deref())
    }

    /// The daemon's measurements, by id.
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

    /// How measurement `m`'s curves draw: its own display, a math channel its owner's (as it
    /// takes its owner's colours), the defaults for anything else.
    pub fn display_of(&self, m: &Measurement) -> ac2_proto::model::SpectrumDisplay {
        if let Some(d) = m.config.kind.display() {
            return *d;
        }
        if let MeasKind::Math { config } = &m.config.kind
            && let Some(d) = config
                .owner
                .meas()
                .and_then(|id| self.meas(id))
                .and_then(|o| o.config.kind.display())
        {
            return *d;
        }
        ac2_proto::model::SpectrumDisplay::default()
    }
}

impl AppState {
    /// What folding the frames of the rig in `rig` takes from the app: what its panes on
    /// screen show of it.
    pub(super) fn folding(&self) -> Folding {
        Folding {
            now_s: self.now_s,
            spl_hold_ms: self.prefs.spl_hold_ms,
            spec_ease: self.prefs.spec_ease,
            advice: self.prefs.advice,
            cooling: self.ember_cooling(),
            pairs_shown: self.programme_pairs_shown(),
            spectrograph_shown: self.spectrograph_shown(),
        }
    }
}
