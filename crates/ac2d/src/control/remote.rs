//! Remote inputs on the control thread (`docs/design/remote-inputs.md`): the links to the
//! paired rigs (`rig.peers`), the source side's filtered state (`source.state`) and tap, the
//! joined feed of a session with remote inputs, and the `remote` entity they report into.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use ac2_core::mic_curve::MicCurve;
use ac2_proto::event::Change;
use ac2_proto::model::{
    BackendKind, CalStatus, InputKind, MeasKind, RemoteAudio, RemoteInput, RemoteInputStatus,
    RemoteLink, RemoteRig, RemoteState, RigPeer, SessionConfig, SourceCurve, SourceInput,
    SourceSession, SourceState, StreamIncarnation, SystemClock, remote_slot,
};
use ac2_proto::units::{RigId, Seconds, SessionEpoch, WallNs};
use ac2_proto::{ProtoError, ReplyBody};
use ac2_zmq::PublicKey;

use crate::calstore::{self, InputCal};
use crate::outbox::Outbox;
use crate::remote::join::{JoinInput, JoinSetup, Joiner};
use crate::remote::link::{self, LinkFrame, LinkHandle};
use crate::remote::peer_clock::{Evidence, PeerClockWatch};
use crate::remote::tap::SourceTap;
use crate::remote::{InputReport, RemoteMsg, RemoteRun, RigReport};
use crate::rig::RigSettings;
use crate::session::Runtime;
use crate::util::{invalid, refused, wall_ns};

/// Audio queued for other rigs while the I/O thread does not take it, s: well past the
/// longest link offset, so a hiccup loses nothing a rig could still use, and no more, since
/// anything older reaches the other rig late.
const TAP_QUEUE_S: f64 = 4.0 * crate::remote::MAX_LINK_OFFSET_S;

use super::Control;
use super::rig::ServerSetup;

/// Frames a link may hold for the fan-out: a second of 64-frame blocks of one channel; past
/// that the fan-out is behind and the audio shows late there.
const LINK_QUEUE: usize = 1024;
/// The most rigs paired with one rig.
const MAX_PEERS: usize = 16;

/// The joined feed's last report.
struct Joined {
    epoch: SessionEpoch,
    inputs: Vec<InputReport>,
    rigs: Vec<RigReport>,
    link_offset: Seconds,
}

/// What the control thread keeps for remote inputs.
pub(super) struct RemoteCtl {
    /// The paired rigs as the rig settings had them, until the links start.
    at_start: Vec<RigPeer>,
    links: BTreeMap<RigId, LinkHandle>,
    /// Each link's last report: state, bytes per second, window.
    link_state: BTreeMap<RigId, (RemoteLink, f64, u32)>,
    pub(super) sources: BTreeMap<RigId, Box<SourceState>>,
    joined: Option<Joined>,
    pub(super) watches: BTreeMap<RigId, PeerClockWatch>,
    /// The running transfer functions between this rig and one remote rig.
    pub(super) cross: BTreeMap<ac2_proto::units::MeasId, super::cross_rig::CrossTf>,
    /// The links feeding the open session.
    fed: BTreeSet<RigId>,
    system_synced: Arc<AtomicBool>,
    started: Instant,
}

impl RemoteCtl {
    pub(super) fn new(peers: Vec<RigPeer>) -> Self {
        Self {
            at_start: peers,
            links: BTreeMap::new(),
            link_state: BTreeMap::new(),
            sources: BTreeMap::new(),
            joined: None,
            watches: BTreeMap::new(),
            cross: BTreeMap::new(),
            fed: BTreeSet::new(),
            system_synced: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
        }
    }
}

/// Checks a `rig.peers` list: each rig once, with a name, endpoints and a server key.
pub(crate) fn check_peers(peers: &[RigPeer]) -> Result<(), String> {
    if peers.len() > MAX_PEERS {
        return Err(format!("{} paired rigs; at most {MAX_PEERS}", peers.len()));
    }
    for (i, p) in peers.iter().enumerate() {
        if p.name.trim().is_empty() || p.name.chars().count() > 64 {
            return Err("a paired rig's name is 1 … 64 characters".into());
        }
        if peers[..i].iter().any(|o| o.rig == p.rig) {
            return Err(format!("{} listed twice", p.name));
        }
        for ep in [&p.ctrl, &p.data] {
            if !ep.starts_with("tcp://") {
                return Err(format!("{}: {ep} is not a tcp:// endpoint", p.name));
            }
        }
        if PublicKey::from_z85(&p.key).is_err() {
            return Err(format!(
                "{}: not a server key (40 Z85 characters expected)",
                p.name
            ));
        }
    }
    Ok(())
}

/// The session channels a job of `kind` reads.
pub(super) fn kind_inputs(kind: &MeasKind) -> Vec<u16> {
    match kind {
        MeasKind::Spectrum { config } => vec![config.input],
        MeasKind::Rta { config } => vec![config.input],
        MeasKind::Spl { config } => vec![config.input],
        MeasKind::Transfer { config } => vec![config.reference_input, config.measurement_input],
        MeasKind::Math { .. } | MeasKind::Sweep { .. } => Vec::new(),
    }
}

/// Whether blocks of `backend` carry the time their samples were captured.
fn capture_stamps(backend: BackendKind) -> bool {
    match backend {
        BackendKind::Jack | BackendKind::CoreAudio | BackendKind::Fake | BackendKind::Replay => {
            true
        }
        BackendKind::Asio => false,
    }
}

/// Why `src` cannot give `channel` to a session at `rate` Hz, as refused at open; `None`
/// when it can, or while its state is not known.
fn source_refusal(src: &SourceState, channel: u16, rate: Option<u32>) -> Option<String> {
    let s = src.session.as_ref()?;
    if s.replaying {
        return Some(format!("{} is replaying a recording", src.name));
    }
    if let Some(ours) = rate
        && ours != s.sample_rate_hz
    {
        return Some(format!(
            "{} runs {} kHz, this rig {} kHz",
            src.name,
            khz(s.sample_rate_hz),
            khz(ours)
        ));
    }
    if !s.inputs.iter().any(|i| i.channel == channel) {
        return Some(format!(
            "{} does not capture in {} (open it there)",
            src.name,
            channel + 1
        ));
    }
    None
}

fn khz(hz: u32) -> String {
    let k = f64::from(hz) / 1000.0;
    if k.fract() == 0.0 {
        format!("{k:.0}")
    } else {
        format!("{k:.1}")
    }
}

impl Control {
    /// Starts the links to the rigs the settings pair with this one.
    pub(super) fn remote_start(&mut self) {
        let peers = std::mem::take(&mut self.remote.at_start);
        self.respawn_links(&peers);
        if self.store.state().remote.peers != peers {
            let mut r = self.store.state().remote.clone();
            r.peers = peers;
            self.commit(Change::Remote(r));
        }
        self.remote_publish();
    }

    /// Starts a link to every peer of `peers` that has none or whose pairing changed, and
    /// ends the others.
    fn respawn_links(&mut self, peers: &[RigPeer]) {
        let keys = match &self.s.server {
            ServerSetup::Network(n) => Some(n.keys.clone()),
            _ => None,
        };
        self.remote
            .links
            .retain(|rig, l| peers.iter().any(|p| p.rig == *rig && *p == l.peer));
        self.remote
            .link_state
            .retain(|rig, _| peers.iter().any(|p| p.rig == *rig));
        self.remote
            .sources
            .retain(|rig, _| peers.iter().any(|p| p.rig == *rig));
        self.remote
            .fed
            .retain(|rig| self.remote.links.contains_key(rig));
        let Some(keys) = keys else {
            return;
        };
        for p in peers {
            if self.remote.links.contains_key(&p.rig) {
                continue;
            }
            match link::spawn(
                p.clone(),
                keys.clone(),
                self.s.ctx.clone(),
                self.s.to_self.clone(),
            ) {
                Ok(h) => {
                    self.remote
                        .link_state
                        .insert(p.rig, (RemoteLink::Connecting, 0.0, 0));
                    self.remote.links.insert(p.rig, h);
                }
                Err(e) => tracing::error!("link to {}: {e}", p.name),
            }
        }
    }

    /// `rig.peers`.
    pub(super) fn set_rig_peers(&mut self, peers: Vec<RigPeer>) -> Result<ReplyBody, ProtoError> {
        check_peers(&peers).map_err(invalid)?;
        if peers.iter().any(|p| p.rig == self.s.rig_id) {
            return Err(invalid("a rig is not paired with itself"));
        }
        if peers != self.store.state().remote.peers {
            self.rig.settings.persist(&RigSettings {
                peers: peers.clone(),
                ..self.rig_settings_now()
            })?;
            self.respawn_links(&peers);
            let mut r = self.store.state().remote.clone();
            r.peers = peers;
            self.commit(Change::Remote(r));
            // A link started again feeds nothing: the session's joined feed starts over.
            if let Some(rt) = &mut self.session {
                rt.remote = None;
            }
            self.remote_attach();
            self.remote_publish();
        }
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }

    /// `source.state`: what a rig taking this rig's inputs may know.
    pub(super) fn source_state(&self) -> SourceState {
        let st = self.store.state();
        let session = self.session.as_ref().map(|rt| SourceSession {
            incarnation: StreamIncarnation {
                daemon: self.s.incarnation,
                epoch: rt.epoch,
            },
            sample_rate_hz: rt.sample_rate,
            buffer_frames: rt.open.buffer_frames,
            replaying: rt.open.replay.is_some(),
            capture_stamps: capture_stamps(rt.open.backend),
            inputs: rt
                .input_map
                .iter()
                .map(|&channel| {
                    let cal = self.input_cal(rt, channel);
                    let setup = st.inputs.iter().find(|i| i.channel == channel).cloned();
                    let curve = cal.curve.as_ref().and_then(|r| {
                        let mic = setup.as_ref()?.mic()?;
                        let c = self.cal.curve(mic, &r.label)?;
                        Some(SourceCurve {
                            reference: r.clone(),
                            freq: c.freqs().to_vec(),
                            gain_db: c.gains().to_vec(),
                        })
                    });
                    SourceInput {
                        channel,
                        setup,
                        cal_status: cal.status,
                        calibration: cal.spl_entry,
                        curve,
                    }
                })
                .collect(),
        });
        SourceState {
            rig: self.s.rig_id,
            name: self.s.name.clone(),
            clocks: st.clocks.clone(),
            session,
        }
    }

    /// Refuses a session the operator opens whose remote inputs cannot be: an input of this
    /// rig, or one its rig is known not to give.
    pub(super) fn check_remote_inputs(&self, cfg: &SessionConfig) -> Result<(), ProtoError> {
        for r in &cfg.remote_inputs {
            if r.rig == self.s.rig_id {
                return Err(invalid("that input is on this rig"));
            }
            if let Some(src) = self.remote.sources.get(&r.rig)
                && let Some(why) = source_refusal(src, r.channel, cfg.sample_rate_hz)
            {
                return Err(refused(why));
            }
        }
        Ok(())
    }

    /// Sets the source tap and the joined feed up on the open stream, once per stream.
    pub(super) fn remote_attach(&mut self) {
        let Some(rt) = self.session.as_ref() else {
            return;
        };
        if rt.remote.is_some() {
            return;
        }
        // The tap: other rigs may take this rig's inputs in network mode only.
        if matches!(self.s.server, ServerSetup::Network(_)) {
            // A message per channel per device period; past this much audio queued the I/O
            // thread has stalled, and the newest frames are dropped rather than memory
            // spent on audio the other rig would take as late anyway.
            let periods = (TAP_QUEUE_S * f64::from(rt.sample_rate)
                / f64::from(rt.open.buffer_frames.max(1)))
            .ceil() as u32;
            let hwm = periods
                .saturating_mul(u32::try_from(rt.input_map.len()).unwrap_or(u32::MAX))
                .max(1);
            match Outbox::connect(&self.s.ctx, &self.s.endpoint, hwm) {
                Ok(o) => rt.fanout.tap(Some(SourceTap::new(
                    o,
                    Arc::clone(&self.s.interest),
                    &rt.input_map,
                    StreamIncarnation {
                        daemon: self.s.incarnation,
                        epoch: rt.epoch,
                    },
                    rt.sample_rate,
                    capture_stamps(rt.open.backend),
                ))),
                Err(e) => tracing::error!("audio for other rigs: no pipe: {e}"),
            }
        }
        let cfg = &rt.open.config;
        let inputs: Vec<RemoteInput> = cfg.remote_inputs.clone();
        let mut joined_map = Vec::new();
        if !inputs.is_empty() {
            joined_map = rt.input_map.clone();
            joined_map.extend(inputs.iter().map(RemoteInput::session_channel));
            let mut rigs = Vec::new();
            let mut feeds = Vec::new();
            for r in &inputs {
                if rigs.iter().any(|(rig, _)| *rig == r.rig) {
                    continue;
                }
                let (tx, rx) = std::sync::mpsc::sync_channel(LINK_QUEUE);
                if let Some(src) = self.remote.sources.get(&r.rig) {
                    let _ = tx.try_send(LinkFrame::Source(src.clone()));
                }
                rigs.push((r.rig, rx));
                let channels: Vec<u16> = inputs
                    .iter()
                    .filter(|o| o.rig == r.rig)
                    .map(|o| o.channel)
                    .collect();
                feeds.push((r.rig, channels, tx));
            }
            let joiner = Joiner::new(JoinSetup {
                inputs: inputs
                    .iter()
                    .map(|r| JoinInput {
                        slot: r.slot,
                        rig: r.rig,
                        channel: r.channel,
                    })
                    .collect(),
                rigs,
                local_columns: rt.input_map.len(),
                rate: rt.sample_rate,
                period: rt.open.buffer_frames.max(1),
                link_offset: cfg.link_offset.map(|s| s.0),
                system_synced: Arc::clone(&self.remote.system_synced),
                to_control: self.s.to_self.clone(),
                epoch: rt.epoch,
            });
            rt.fanout.join(Some(joiner));
            for (rig, channels, tx) in feeds {
                if let Some(l) = self.remote.links.get(&rig) {
                    l.feed(channels, Some(tx));
                    self.remote.fed.insert(rig);
                }
            }
        }
        self.remote.joined = None;
        if let Some(rt) = &mut self.session {
            rt.remote = Some(RemoteRun { joined_map });
        }
        self.remote_publish();
    }

    /// Stops the links feeding a session that is gone or has no remote inputs from them.
    pub(super) fn remote_sync(&mut self) {
        if self.remote.fed.is_empty() {
            return;
        }
        let wanted: BTreeSet<RigId> = self
            .session
            .as_ref()
            .filter(|rt| rt.remote.is_some())
            .map(|rt| rt.open.config.remote_inputs.iter().map(|r| r.rig).collect())
            .unwrap_or_default();
        let stale: Vec<RigId> = self.remote.fed.difference(&wanted).copied().collect();
        for rig in stale {
            if let Some(l) = self.remote.links.get(&rig) {
                l.feed(Vec::new(), None);
            }
            self.remote.fed.remove(&rig);
        }
        if wanted.is_empty() && self.remote.joined.take().is_some() {
            self.remote_publish();
        }
    }

    pub(super) fn remote_msg(&mut self, m: RemoteMsg) {
        match m {
            RemoteMsg::Link {
                rig,
                link,
                bytes_per_s,
                window,
            } => {
                if self.remote.links.contains_key(&rig) {
                    self.remote
                        .link_state
                        .insert(rig, (link, bytes_per_s, window));
                }
            }
            RemoteMsg::Source { rig, state } => {
                if !self.remote.links.contains_key(&rig) {
                    return;
                }
                let inputs_changed = self
                    .remote
                    .sources
                    .get(&rig)
                    .map(|s| s.session.as_ref().map(|s| &s.inputs))
                    != Some(state.session.as_ref().map(|s| &s.inputs));
                self.remote.sources.insert(rig, state);
                if inputs_changed {
                    // Jobs on its inputs apply its calibration and curve as it does.
                    self.refresh_cal();
                }
            }
            RemoteMsg::Joined {
                epoch,
                inputs,
                rigs,
                link_offset,
            } => {
                if self.session.as_ref().is_some_and(|rt| rt.epoch == epoch) {
                    self.remote.joined = Some(Joined {
                        epoch,
                        inputs,
                        rigs,
                        link_offset,
                    });
                }
            }
        }
        self.remote_publish();
    }

    /// The `remote` entity from what the links and the joined feed reported; committed when
    /// it changed.
    pub(super) fn remote_publish(&mut self) {
        let st = self.store.state();
        let ours = st.clocks.clone();
        self.remote.system_synced.store(
            matches!(ours.system, SystemClock::Synced { .. }),
            Ordering::Relaxed,
        );
        let network = matches!(self.s.server, ServerSetup::Network(_));
        let peers = st.remote.peers.clone();
        let session_inputs: Vec<RemoteInput> = self
            .session
            .as_ref()
            .map(|rt| rt.open.config.remote_inputs.clone())
            .unwrap_or_default();
        let epoch = self.session.as_ref().map(|rt| rt.epoch);
        let joined = self
            .remote
            .joined
            .as_ref()
            .filter(|j| Some(j.epoch) == epoch);
        let now = self.remote.started.elapsed();
        let wall = WallNs(wall_ns());
        let mut rigs: Vec<RemoteRig> = Vec::new();
        for r in &session_inputs {
            if rigs.iter().any(|x| x.rig == r.rig) {
                continue;
            }
            let paired = peers.iter().find(|p| p.rig == r.rig);
            let (link, bytes_per_s, window) = match (network, paired) {
                (false, _) => (RemoteLink::NotNetworked, 0.0, 0),
                (true, None) => (RemoteLink::NotPaired, 0.0, 0),
                (true, Some(_)) => self.remote.link_state.get(&r.rig).cloned().unwrap_or((
                    RemoteLink::Connecting,
                    0.0,
                    0,
                )),
            };
            let src = self.remote.sources.get(&r.rig);
            let report = joined.and_then(|j| j.rigs.iter().find(|x| x.rig == r.rig));
            let evidence = Evidence {
                ours: (self.s.rig_id, &ours),
                theirs: (r.rig, src.map(|s| &s.clocks)),
                capture_stamps: src
                    .and_then(|s| s.session.as_ref())
                    .is_some_and(|s| s.capture_stamps)
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|rt| capture_stamps(rt.open.backend)),
                long: report.and_then(|x| x.long),
                short: report.and_then(|x| x.short),
                streaming_for: report.and_then(|x| x.streaming_for),
                disagree: report.and_then(|x| x.disagree),
            };
            let watch = self.remote.watches.entry(r.rig).or_default();
            watch.update(&evidence, now, wall);
            rigs.push(RemoteRig {
                rig: r.rig,
                name: src.map_or_else(|| r.rig_name.clone(), |s| s.name.clone()),
                link,
                clock: watch.clock.clone(),
                bytes_per_s,
                window,
            });
        }
        let in_session: BTreeSet<RigId> = rigs.iter().map(|r| r.rig).collect();
        self.remote
            .watches
            .retain(|rig, _| in_session.contains(rig));
        let inputs: Vec<RemoteInputStatus> = session_inputs
            .iter()
            .map(|r| {
                let rep = joined.and_then(|j| j.inputs.iter().find(|i| i.slot == r.slot));
                let src_input = self
                    .remote
                    .sources
                    .get(&r.rig)
                    .and_then(|s| s.session.as_ref())
                    .and_then(|s| s.inputs.iter().find(|i| i.channel == r.channel));
                let audio = if r.rig == self.s.rig_id {
                    RemoteAudio::OnThisRig
                } else {
                    rep.map_or(RemoteAudio::Waiting, |x| x.audio)
                };
                RemoteInputStatus {
                    slot: r.slot,
                    audio,
                    anchor: rep.and_then(|x| x.anchor),
                    late_frames: rep.map_or(0, |x| x.late_frames),
                    late_frames_hour: rep.map_or(0, |x| x.late_frames_hour),
                    slips: rep.map_or(0, |x| x.slips),
                    last_slip: rep.and_then(|x| x.last_slip),
                    arrival_margin: rep.and_then(|x| x.arrival_margin),
                    setup: src_input.and_then(|i| i.setup.clone()),
                    cal_status: src_input.map_or(CalStatus::Uncalibrated, |i| i.cal_status),
                    calibration: src_input.and_then(|i| i.calibration.clone()),
                    curve: src_input.and_then(|i| i.curve.as_ref().map(|c| c.reference.clone())),
                }
            })
            .collect();
        let link_offset = joined.map(|j| j.link_offset);
        self.cross_sync(&inputs, link_offset);
        let transfers = self.cross_entries();
        let next = RemoteState {
            peers,
            rigs,
            inputs,
            link_offset: if session_inputs.is_empty() {
                None
            } else {
                link_offset
            },
            transfers,
        };
        if next != self.store.state().remote {
            self.commit(Change::Remote(next));
        }
    }

    /// The remote input of session channel `ch`, if it is one.
    fn remote_input(&self, ch: u16) -> Option<&RemoteInput> {
        let slot = remote_slot(ch)?;
        self.session
            .as_ref()?
            .open
            .config
            .remote_inputs
            .iter()
            .find(|r| r.slot == slot)
    }

    /// The calibration and curve a job on remote channel `ch` applies: its own rig's.
    pub(super) fn remote_input_cal(&self, ch: u16) -> InputCal {
        let Some(r) = self.remote_input(ch) else {
            return InputCal::none();
        };
        let Some(i) = self
            .remote
            .sources
            .get(&r.rig)
            .and_then(|s| s.session.as_ref())
            .and_then(|s| s.inputs.iter().find(|i| i.channel == r.channel))
        else {
            return InputCal::none();
        };
        let correction = i.curve.as_ref().and_then(|c| {
            let points: Vec<(f64, f64)> = c
                .freq
                .iter()
                .copied()
                .zip(c.gain_db.iter().copied())
                .collect();
            let curve = MicCurve::from_points(&points).ok()?;
            Some(Arc::new(
                curve.normalised(calstore::f_norm(i.calibration.as_ref())),
            ))
        });
        InputCal {
            status: i.cal_status,
            sensitivity: i.calibration.as_ref().map(|e| e.spl.sensitivity.0),
            correction,
            spl_entry: i.calibration.clone(),
            curve: i.curve.as_ref().map(|c| c.reference.clone()),
        }
    }

    /// What is on remote channel `ch`, as its rig declares it.
    pub(super) fn remote_input_kind(&self, ch: u16) -> Option<InputKind> {
        let r = self.remote_input(ch)?;
        let setup = self
            .remote
            .sources
            .get(&r.rig)?
            .session
            .as_ref()?
            .inputs
            .iter()
            .find(|i| i.channel == r.channel)?
            .setup
            .as_ref()?;
        Some(setup.kind.clone())
    }

    /// Refuses a change on remote channel `ch` that belongs to its own rig.
    pub(super) fn not_remote(&self, ch: u16) -> Result<(), ProtoError> {
        if remote_slot(ch).is_none() {
            return Ok(());
        }
        let name = self
            .remote_input(ch)
            .map_or_else(|| "its rig".to_owned(), |r| r.rig_name.clone());
        Err(refused(format!("set on {name}")))
    }

    /// The block map a job on `inputs` reads: this rig's own, or the joined feed's when any
    /// of them is remote. `Err` for a transfer function on two different remote rigs: its
    /// clocks would have to agree with each other through this rig's, not measured yet.
    pub(super) fn job_map(
        &self,
        rt: &Runtime,
        inputs: &[u16],
        transfer: bool,
    ) -> Result<Option<Vec<u16>>, ProtoError> {
        let mut remote: Vec<&RemoteInput> = Vec::new();
        for &c in inputs {
            if let Some(slot) = remote_slot(c) {
                remote.push(self.remote_input(c).ok_or_else(|| {
                    invalid(format!("remote input {slot} is not in the session"))
                })?);
            }
        }
        if remote.is_empty() {
            return Ok(None);
        }
        if transfer
            && remote
                .iter()
                .map(|r| r.rig)
                .collect::<BTreeSet<RigId>>()
                .len()
                > 1
        {
            return Err(refused("both inputs remote on different rigs: not yet"));
        }
        Ok(Some(
            rt.remote
                .as_ref()
                .map(|r| r.joined_map.clone())
                .unwrap_or_default(),
        ))
    }

    /// The remote session channels, for the session input meters.
    pub(super) fn remote_channels(rt: &Runtime) -> Vec<u16> {
        rt.open
            .config
            .remote_inputs
            .iter()
            .map(RemoteInput::session_channel)
            .collect()
    }
}
