//! What the app holds for one daemon: its link, its mirror and data, and every choice keyed
//! by its ids.

use super::*;

/// The id of a rig whose daemon has not said hello yet. No daemon has it: `ac2d` makes its
/// id at random and never 0.
pub const UNKNOWN_RIG: RigId = RigId(0);

/// The newest frame of one topic a fold took in. A daemon numbers each topic's frames
/// (`seq`) for as long as it runs, from 1: a frame of another incarnation (the daemon
/// restarted, or another one answers at the address) starts a count of its own, however
/// small its `seq`, and is never an old frame. A link that drops and reaches the same
/// daemon again finds its count going on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldedAt {
    pub incarnation: ac2_proto::units::DaemonIncarnation,
    pub seq: u64,
}

/// What a frame is to a fold over its topic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextFrame {
    /// Folded in already, or older than what was.
    Seen,
    /// The next of the stream folded so far.
    Newer,
    /// The first frame, or the first of another incarnation.
    Afresh,
}

impl FoldedAt {
    pub fn of(s: &ac2_proto::frame::FrameStamp) -> Self {
        Self {
            incarnation: s.daemon_incarnation,
            seq: s.seq,
        }
    }

    /// What frame `s` is to a fold that last took `last` (`None`: none yet).
    pub fn next(last: Option<Self>, s: &ac2_proto::frame::FrameStamp) -> NextFrame {
        match last {
            Some(l) if l.incarnation == s.daemon_incarnation => {
                if s.seq > l.seq {
                    NextFrame::Newer
                } else {
                    NextFrame::Seen
                }
            }
            _ => NextFrame::Afresh,
        }
    }
}

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
    /// What the frames so far leave behind to draw with: eases, holds, histories, caches.
    pub folded: Folded,
    /// Stored traces' data, as fetched.
    pub traces: BTreeMap<TraceId, (Arc<TraceData>, Arc<GridDef>)>,
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
    pub stimulus: Stimulus,
    pub sweep: SweepUi,
    /// The output device the stimulus outputs belong to (the open session's).
    pub(super) stim_device: Option<String>,
    /// Band and observation X / Shift+X run the finder with.
    pub finder: FinderChoice,
    /// The app stops this rig's daemon when it quits and that daemon keeps nothing (the
    /// simulated rig): quitting with unsaved changes asks first.
    pub quit_loses: bool,
    /// What waits for the mirror to list it.
    pub(super) pending: Pending,
    /// Follow-ups waiting for the daemon's answer or its next state.
    pub(crate) due: Due,
    /// The measurement chosen, its own panes on screen, and its stand-in when it goes.
    pub(super) choice: MeasChoice,
    /// What the daemon was last asked for, so it is asked again only when that changes.
    pub(crate) asked: LastAsked,
    /// The daemon's backends and devices as last listed (channel names for the dialogs).
    pub devices: Option<Vec<ac2_proto::model::BackendInfo>>,
    /// The daemon's last `server.info` answer (or why it could not be asked): its mode and
    /// key, which pairing it with another rig gives that rig.
    pub server: Option<Result<ac2_proto::model::ServerInfo, String>>,
    /// The SPL meters' Leq history, strips, logs and alarms.
    pub leq: LeqUi,
    /// The Programme panes' flying peaks and ember rings.
    pub programme: ProgrammeUi,
    /// The Inputs-list row picked (its input, a pair's left): the keys go to its meters
    /// until a pane or another row is clicked.
    pub input_pick: Option<u16>,
    /// `meas.layout` calls in flight and the edits waiting for them.
    pub(super) layout_queue: LayoutQueue,
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
    /// Measurements whose live curve a pane picked ([`AppState::peak_points_of`]): their
    /// peaks are tracked whatever their mode.
    pub peak_picked: BTreeSet<MeasId>,
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
            daemon_addr: None,
            server: None,
            duplicate: false,
            phase_reference: None,
            conn: ConnState::Connecting { target },
            mirror: None,
            data: None,
            folded: Folded::default(),
            traces: BTreeMap::new(),
            selected: None,
            selected_trace: None,
            selected_live: None,
            hidden_meas: BTreeSet::new(),
            collapsed: BTreeSet::new(),
            edits: BTreeMap::new(),
            stimulus: Stimulus::default(),
            sweep: SweepUi::default(),
            stim_device: None,
            finder: FinderChoice::default(),
            quit_loses: false,
            pending: Pending::default(),
            due: Due::default(),
            choice: MeasChoice::default(),
            asked: LastAsked::default(),
            devices: None,
            leq: LeqUi::default(),
            programme: ProgrammeUi::default(),
            input_pick: None,
            layout_queue: LayoutQueue::default(),
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
        self.fold_peak_tracks(&d, &fold.peak_picked);
        if fold.spectrograph_shown {
            self.fold_spectrographs(&d, fold.spec_ease);
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

    /// This connection's identity on the rig ([`AppState::my_client_id`]).
    pub fn client_id(&self) -> Option<&ClientId> {
        let ConnState::Connected { client_id, .. } = &self.conn else {
            return None;
        };
        match &self.mirror {
            Some(m) if m.incarnation.is_some() => m.client_id.as_ref(),
            _ => Some(client_id),
        }
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
            peak_picked: self.peak_picked(),
        }
    }
}
