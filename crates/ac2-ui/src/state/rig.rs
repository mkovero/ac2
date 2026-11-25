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
    /// a daemon that is another rig each run).
    pub addr: Option<crate::prefs::RigAddr>,
    /// The host the link reaches the daemon at over the network; `None` on a local link.
    pub daemon_addr: Option<String>,
    /// The daemon said hello as this rig, which the app already holds in another slot
    /// (the same daemon reached at another address): the app closes this link.
    pub duplicate: Option<RigId>,
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
    /// The slots (bit N for slot N) a Ctrl+N capture is in flight for: a second press waits
    /// for the first's answer, so the two never strip each other's new traces of the slot.
    pub(crate) capturing: u16,
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
    /// Dynamics runs as this client runs them.
    pub dynamics_run: super::dynamics_run::DynamicsRunUi,
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
    /// The dynamics runs' step tables and the envelopes the panes asked for.
    pub dynamics: super::dynamics::DynamicsUi,
    /// The Inputs-list row picked (its input, a pair's left): the keys go to its meters
    /// until a pane or another row is clicked.
    pub input_pick: Option<u16>,
    /// `meas.layout` calls in flight and the edits waiting for them.
    pub(super) layout_queue: LayoutQueue,
    /// The file on this machine this app last opened the working state from or exported
    /// it to ([`OwnFile`]).
    pub(super) own_file: Option<OwnFile>,
}

/// A session file on this machine that this app moved the working state through. The
/// daemon names a client's file by its name only, which a file on another machine can
/// share; the save time ties the file to the one working state it was opened as or
/// exported from, so Save writes over it only while that state is the daemon's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OwnFile {
    pub path: std::path::PathBuf,
    /// The working state's save time; `None` until the daemon's state shows the export.
    pub saved_at: Option<ac2_proto::units::WallNs>,
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
    /// Transfer columns with γ² below this are not drawn, so a transfer's peak hold takes
    /// nothing from them.
    pub blank_below: Option<f32>,
}

impl RigState {
    /// `link` in place of its link, which is handed back to be closed: a new link was asked
    /// for nothing yet, so every standing request goes out again on it.
    pub(crate) fn relinked(&mut self, link: crate::conn::Conn) -> Option<crate::conn::Conn> {
        self.asked = LastAsked::default();
        self.link.replace(link)
    }

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
            duplicate: None,
            phase_reference: None,
            conn: ConnState::Connecting { target },
            mirror: None,
            data: None,
            folded: Folded::default(),
            traces: BTreeMap::new(),
            selected: None,
            selected_trace: None,
            capturing: 0,
            selected_live: None,
            hidden_meas: BTreeSet::new(),
            collapsed: BTreeSet::new(),
            edits: BTreeMap::new(),
            stimulus: Stimulus::default(),
            sweep: SweepUi::default(),
            dynamics_run: super::dynamics_run::DynamicsRunUi::default(),
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
            dynamics: super::dynamics::DynamicsUi::default(),
            input_pick: None,
            layout_queue: LayoutQueue::default(),
            own_file: None,
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
        self.fold_peaks(&d, fold.blank_below);
        self.fold_peak_tracks(&d);
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
        self.drop_outdated_frames();
        forgot
    }

    /// Drops the frames of each stopped measurement whose newest result is of an older
    /// configuration than the measurement's own (it was reset, given a new delay or new
    /// settings while stopped): the daemon no longer keeps that result, a capture refuses
    /// it, so the views show the measurement as stopped with nothing measured.
    pub(super) fn drop_outdated_frames(&mut self) {
        let Some(state) = self.mirror.as_ref().and_then(|m| m.state.as_deref()) else {
            return;
        };
        let Some(d) = &self.data else {
            return;
        };
        let meas_of = |t: &Topic| match t {
            Topic::Data { meas, .. } | Topic::Bins { meas, .. } => Some(*meas),
            _ => None,
        };
        let outdated: Vec<MeasId> = state
            .measurements
            .iter()
            .filter(|m| !m.running)
            .filter(|m| {
                let newest = d
                    .latest
                    .frames
                    .values()
                    .filter(|f| matches!(f.topic, Topic::Data { meas, .. } if meas == m.id))
                    .map(|f| f.frame.stamp.config_rev)
                    .max();
                newest.is_some_and(|r| r < m.config_rev)
            })
            .map(|m| m.id)
            .collect();
        if outdated.is_empty() {
            return;
        }
        let mut nd = (**d).clone();
        nd.latest
            .frames
            .retain(|_, f| !meas_of(&f.topic).is_some_and(|m| outdated.contains(&m)));
        self.data = Some(Arc::new(nd));
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

    /// How measurement `m`'s curves draw: its own display, a math trace its owner's (as it
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

    /// What P draws over measurement `m`'s live curve: its own peak mode, a math trace
    /// its owner's (as it takes its owner's display), off for a kind that takes none.
    pub fn peak_of(&self, m: &Measurement) -> ac2_proto::model::PeakMode {
        if let Some(p) = m.config.kind.peak() {
            return p;
        }
        if let MeasKind::Math { config } = &m.config.kind
            && let Some(p) = config
                .owner
                .meas()
                .and_then(|id| self.meas(id))
                .and_then(|o| o.config.kind.peak())
        {
            return p;
        }
        ac2_proto::model::PeakMode::Off
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
            blank_below: self.view.tf.coherence.blank_below,
        }
    }
}
