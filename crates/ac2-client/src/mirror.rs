//! Mirrored daemon state (Q5) as a pure state machine.
//!
//! [`Mirror`] consumes keepalives, events and the replies to `state.snapshot` /
//! `state.since`, and says which request it needs next ([`Need`]). It never performs I/O, so
//! every path of the sync procedure is testable without sockets; the client's sync task
//! (`client.rs`) feeds it and executes its needs.
//!
//! Procedure (docs/design/q2-q5-q6-protocol.md):
//! 1. `evt` and `ka` are subscribed before anything else; events are buffered.
//! 2. The first `ka` proves the subscription live → snapshot (rev R).
//! 3. Buffered events with rev > R are applied in order, then live ones.
//! 4. An event with rev > last + 1 → `state.since(last)`; `resync_required` → snapshot.
//! 5. A `ka` of another incarnation → drop everything and resync from step 2.
//! 6. `ka.rev` ahead of the last applied rev for more than 1 s → `state.since` (catches a
//!    missed final patch).

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_proto::frame::{GenSummary, KaMeta};
use ac2_proto::model::{State, TimingState};
use ac2_proto::units::{ClientId, DaemonIncarnation, Rev, SessionEpoch};
use ac2_proto::{Change, Event, FrameStamp, Patch, StateSnapshot};

/// No `ka` for this long: the daemon is not responding.
pub const KA_TIMEOUT: Duration = Duration::from_millis(1500);
/// `ka.rev` ahead of the applied rev for this long: fetch the missed events.
pub const REV_AHEAD_GRACE: Duration = Duration::from_secs(1);
/// Window of the clock-offset estimate.
pub const CLOCK_WINDOW: Duration = Duration::from_secs(10);
/// Least time between two identical sync requests after a failure.
pub const RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// What the mirror needs from the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// `state.snapshot`.
    Snapshot,
    /// `state.since(rev)`.
    Since(Rev),
    /// The daemon restarted: `hello` again (new client id, new dedup scope), then snapshot.
    Rehello,
}

/// Where the procedure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Subscribed; waiting for the first `ka`.
    AwaitKa,
    /// A snapshot is needed (or in flight).
    NeedSnapshot,
    /// Mirroring.
    Live,
}

/// Clock offset `daemon − local`, estimated from keepalives.
///
/// Each `ka` gives `daemon_wall_ns − local_receive_ns = θ − d`, where θ is the true offset
/// and d ≥ 0 the delivery delay (queueing, scheduling). Every sample is therefore a lower
/// bound on θ, and the tightest one — the largest sample, i.e. the least-delayed `ka` — over
/// the window is the estimate; queueing only ever lowers samples and so never moves it.
#[derive(Debug, Clone, Default)]
pub struct ClockEstimator {
    samples: VecDeque<(Instant, i128)>,
}

impl ClockEstimator {
    /// Adds one keepalive sample.
    pub fn add(&mut self, at: Instant, daemon_wall_ns: u64, local_wall_ns: i128) {
        self.samples
            .push_back((at, i128::from(daemon_wall_ns) - local_wall_ns));
        while let Some((t, _)) = self.samples.front() {
            if at.saturating_duration_since(*t) > CLOCK_WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Estimated `daemon − local` offset, ns.
    pub fn offset_ns(&self) -> Option<i128> {
        self.samples.iter().map(|(_, s)| *s).max()
    }
}

/// Published view of the mirror.
#[derive(Debug, Clone)]
pub struct MirrorView {
    /// Procedure phase.
    pub phase: Phase,
    /// Incarnation of the daemon the state belongs to.
    pub incarnation: Option<DaemonIncarnation>,
    /// Newest session epoch seen (ka, snapshot or session event).
    pub session_epoch: Option<SessionEpoch>,
    /// Last applied rev.
    pub rev: Rev,
    /// Mirrored state; `None` until the first snapshot (and after an incarnation change).
    pub state: Option<Arc<State>>,
    /// When the last `ka` arrived.
    pub last_ka: Option<Instant>,
    /// Generator summary from the last `ka`.
    pub generator: Option<GenSummary>,
    /// Timing state from the last `ka`.
    pub timing: Option<TimingState>,
    /// `ka.rev` of the last `ka`.
    pub ka_rev: Option<Rev>,
    /// Clock offset `daemon − local`, ns.
    pub clock_offset_ns: Option<i128>,
    /// Snapshots taken.
    pub snapshots: u64,
    /// `state.since` requests made (gaps and missed final patches).
    pub since_requests: u64,
    /// Daemon restarts seen.
    pub incarnation_changes: u64,
    /// The identity the daemon bound to this connection, from the `welcome` of the
    /// incarnation shown; `None` while a restarted daemon has not answered `hello` yet. A
    /// restarted daemon binds a new identity, so "owned by me" must be judged with this,
    /// never with an id kept from an earlier connect.
    pub client_id: Option<ClientId>,
}

impl MirrorView {
    /// The state is mirrored and current as far as the client knows.
    pub fn synced(&self) -> bool {
        self.phase == Phase::Live && self.state.is_some()
    }

    /// A keepalive arrived within [`KA_TIMEOUT`] of `now`.
    pub fn responding(&self, now: Instant) -> bool {
        self.last_ka
            .is_some_and(|t| now.saturating_duration_since(t) < KA_TIMEOUT)
    }
}

/// The sync state machine.
#[derive(Debug)]
pub struct Mirror {
    enabled: bool,
    phase: Phase,
    incarnation: Option<DaemonIncarnation>,
    epoch: Option<SessionEpoch>,
    rev: Rev,
    state: Option<Arc<State>>,
    /// Events not yet applicable (before the snapshot, or after a gap), by rev.
    pending: BTreeMap<Rev, Event>,
    ahead_since: Option<Instant>,
    last_request: Option<(Need, Instant)>,
    last_ka: Option<Instant>,
    ka: Option<KaMeta>,
    clock: ClockEstimator,
    snapshots: u64,
    since_requests: u64,
    incarnation_changes: u64,
}

impl Mirror {
    /// A mirror. With `enabled = false` only keepalives are tracked (liveness, clock,
    /// incarnation); no snapshot is ever requested.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            phase: Phase::AwaitKa,
            incarnation: None,
            epoch: None,
            rev: Rev(0),
            state: None,
            pending: BTreeMap::new(),
            ahead_since: None,
            last_request: None,
            last_ka: None,
            ka: None,
            clock: ClockEstimator::default(),
            snapshots: 0,
            since_requests: 0,
            incarnation_changes: 0,
        }
    }

    /// Current view.
    pub fn view(&self) -> MirrorView {
        MirrorView {
            phase: self.phase,
            incarnation: self.incarnation,
            session_epoch: self.epoch,
            rev: self.rev,
            state: self.state.clone(),
            last_ka: self.last_ka,
            generator: self.ka.as_ref().map(|k| k.generator.clone()),
            timing: self.ka.as_ref().map(|k| k.timing),
            ka_rev: self.ka.as_ref().map(|k| k.rev),
            clock_offset_ns: self.clock.offset_ns(),
            snapshots: self.snapshots,
            since_requests: self.since_requests,
            incarnation_changes: self.incarnation_changes,
            client_id: None,
        }
    }

    fn bump_epoch(&mut self, e: SessionEpoch) {
        if self.epoch.is_none_or(|cur| e > cur) {
            self.epoch = Some(e);
        }
    }

    fn request(&mut self, need: Need, now: Instant) -> Option<Need> {
        if matches!(need, Need::Since(_)) {
            self.since_requests += 1;
        }
        self.last_request = Some((need, now));
        Some(need)
    }

    /// A keepalive arrived.
    pub fn on_ka(
        &mut self,
        stamp: &FrameStamp,
        meta: KaMeta,
        now: Instant,
        local_wall_ns: i128,
    ) -> Option<Need> {
        self.clock.add(now, meta.daemon_wall_ns.0, local_wall_ns);
        self.last_ka = Some(now);
        let ka_rev = meta.rev;
        self.ka = Some(meta);
        if self.incarnation != Some(stamp.daemon_incarnation) {
            let restarted = self.incarnation.is_some();
            self.incarnation = Some(stamp.daemon_incarnation);
            self.epoch = Some(stamp.session_epoch);
            self.state = None;
            self.rev = Rev(0);
            self.pending.clear();
            self.ahead_since = None;
            if restarted {
                self.incarnation_changes += 1;
            }
            if !self.enabled {
                return None;
            }
            self.phase = Phase::NeedSnapshot;
            let need = if restarted {
                Need::Rehello
            } else {
                Need::Snapshot
            };
            return self.request(need, now);
        }
        self.bump_epoch(stamp.session_epoch);
        if !self.enabled {
            return None;
        }
        match self.phase {
            Phase::AwaitKa => {
                self.phase = Phase::NeedSnapshot;
                self.request(Need::Snapshot, now)
            }
            Phase::NeedSnapshot => self.retry(now),
            Phase::Live => {
                if ka_rev > self.rev {
                    let since = *self.ahead_since.get_or_insert(now);
                    if now.saturating_duration_since(since) > REV_AHEAD_GRACE {
                        self.ahead_since = Some(now);
                        return self.request(Need::Since(self.rev), now);
                    }
                } else {
                    self.ahead_since = None;
                }
                None
            }
        }
    }

    /// An event arrived on `evt`.
    pub fn on_event(&mut self, ev: Event, now: Instant) -> Option<Need> {
        if !self.enabled {
            return None;
        }
        if self.phase != Phase::Live {
            self.pending.insert(ev.rev, ev);
            return None;
        }
        if ev.rev <= self.rev {
            return None;
        }
        self.pending.insert(ev.rev, ev);
        self.apply_pending(now)
    }

    /// The reply to `state.snapshot`.
    pub fn on_snapshot(&mut self, snap: StateSnapshot, now: Instant) -> Option<Need> {
        if !self.enabled {
            return None;
        }
        if self
            .incarnation
            .is_some_and(|i| i != snap.daemon_incarnation)
        {
            // Taken from another incarnation than the keepalives say: the daemon restarted
            // between the two; the next `ka` decides.
            self.phase = Phase::NeedSnapshot;
            return None;
        }
        self.incarnation = Some(snap.daemon_incarnation);
        self.bump_epoch(snap.session_epoch);
        self.bump_epoch(snap.state.session.epoch);
        self.rev = snap.rev;
        self.state = Some(Arc::new(snap.state));
        self.phase = Phase::Live;
        self.snapshots += 1;
        self.ahead_since = None;
        self.last_request = None;
        let r = self.rev;
        self.pending.retain(|rev, _| *rev > r);
        self.apply_pending(now)
    }

    /// The events of a `state.since` reply.
    pub fn on_since(&mut self, events: Vec<Event>, now: Instant) -> Option<Need> {
        if !self.enabled || self.phase != Phase::Live {
            return None;
        }
        for ev in events {
            if ev.rev > self.rev {
                self.pending.insert(ev.rev, ev);
            }
        }
        self.last_request = None;
        self.apply_pending(now)
    }

    /// `state.since` answered `resync_required`.
    pub fn on_resync_required(&mut self, now: Instant) -> Option<Need> {
        if !self.enabled {
            return None;
        }
        self.phase = Phase::NeedSnapshot;
        self.request(Need::Snapshot, now)
    }

    /// Periodic check: retries failed requests, and the 1 s missed-patch rule.
    pub fn on_tick(&mut self, now: Instant) -> Option<Need> {
        if !self.enabled {
            return None;
        }
        match self.phase {
            Phase::AwaitKa => None,
            Phase::NeedSnapshot => self.retry(now),
            Phase::Live => {
                if !self.pending.is_empty() {
                    return self.retry_need(Need::Since(self.rev), now);
                }
                let ka_rev = self.ka.as_ref().map(|k| k.rev)?;
                if ka_rev > self.rev
                    && self
                        .ahead_since
                        .is_some_and(|s| now.saturating_duration_since(s) > REV_AHEAD_GRACE)
                {
                    self.ahead_since = Some(now);
                    return self.request(Need::Since(self.rev), now);
                }
                None
            }
        }
    }

    /// A requested call failed (timeout, transport); it is retried on a later tick.
    pub fn on_request_failed(&mut self, now: Instant) {
        if let Some((n, _)) = self.last_request {
            self.last_request = Some((n, now));
        }
    }

    fn retry(&mut self, now: Instant) -> Option<Need> {
        self.retry_need(Need::Snapshot, now)
    }

    fn retry_need(&mut self, need: Need, now: Instant) -> Option<Need> {
        match self.last_request {
            Some((_, t)) if now.saturating_duration_since(t) < RETRY_INTERVAL => None,
            _ => self.request(need, now),
        }
    }

    fn apply_pending(&mut self, now: Instant) -> Option<Need> {
        while let Some(entry) = self.pending.first_entry() {
            let rev = *entry.key();
            if rev <= self.rev {
                entry.remove();
            } else if rev.0 == self.rev.0 + 1 {
                let ev = entry.remove();
                self.apply(ev);
            } else {
                // Gap: rev > last + 1.
                return self.retry_need(Need::Since(self.rev), now);
            }
        }
        if self.ka.as_ref().is_some_and(|k| k.rev <= self.rev) {
            self.ahead_since = None;
        }
        None
    }

    fn apply(&mut self, ev: Event) {
        self.rev = ev.rev;
        if let Change::Session(s) = &ev.change {
            self.bump_epoch(s.epoch);
        }
        if let Some(state) = self.state.as_mut() {
            apply_change(Arc::make_mut(state), ev.change);
        }
    }
}

fn upsert<T, K: PartialEq>(v: &mut Vec<T>, patch: Patch<T, K>, key: impl Fn(&T) -> K) {
    match patch {
        Patch::Set(x) => {
            let k = key(&x);
            match v.iter_mut().find(|e| key(e) == k) {
                Some(slot) => *slot = x,
                None => v.push(x),
            }
        }
        Patch::Deleted(k) => v.retain(|e| key(e) != k),
    }
}

/// Applies one change to a state: assignment of the entity's full new value.
pub fn apply_change(s: &mut State, c: Change) {
    match c {
        Change::Session(x) => s.session = x,
        Change::Measurement(p) => upsert(&mut s.measurements, p, |m| m.id),
        Change::Trace(p) => upsert(&mut s.traces, p, |t| t.id),
        Change::Generator(g) => s.generator = g,
        Change::Calibration(p) => upsert(&mut s.calibrations, p, |c| c.key.clone()),
        Change::Inputs(i) => s.inputs = i.clone(),
        Change::SplLog(p) => upsert(&mut s.spl_logs, p, |l| l.meas),
        Change::Timing(t) => s.timing = t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::Measurement;
    use ac2_proto::samples;
    use ac2_proto::units::{MeasId, WallNs};

    fn ka(inc: u64, rev: u64) -> (FrameStamp, KaMeta) {
        let mut stamp = samples::stamp(None);
        stamp.daemon_incarnation = DaemonIncarnation(inc);
        let meta = KaMeta {
            rev: Rev(rev),
            daemon_wall_ns: WallNs(1_000),
            timing: TimingState::NoStimulus,
            generator: GenSummary {
                owner: None,
                armed: false,
                firing: false,
            },
        };
        (stamp, meta)
    }

    fn snap(inc: u64, rev: u64) -> StateSnapshot {
        StateSnapshot {
            state: samples::state(),
            rev: Rev(rev),
            daemon_incarnation: DaemonIncarnation(inc),
            session_epoch: SessionEpoch(2),
        }
    }

    fn meas_ev(rev: u64, id: u32) -> Event {
        let mut m: Measurement = samples::state().measurements[0].clone();
        m.id = MeasId(id);
        Event {
            rev: Rev(rev),
            change: Change::Measurement(Patch::Set(m)),
        }
    }

    #[test]
    fn buffered_events_after_snapshot_and_gap() {
        let t0 = Instant::now();
        let mut m = Mirror::new(true);
        assert_eq!(m.on_event(meas_ev(10, 7), t0), None);
        let (s, k) = ka(1, 10);
        assert_eq!(m.on_ka(&s, k, t0, 0), Some(Need::Snapshot));
        assert_eq!(m.on_event(meas_ev(11, 8), t0), None);
        // Snapshot at 10: event 10 is already in it, 11 applies.
        assert_eq!(m.on_snapshot(snap(1, 10), t0), None);
        let v = m.view();
        assert!(v.synced());
        assert_eq!(v.rev, Rev(11));
        let st = v.state.unwrap_or_else(|| panic!("state"));
        assert!(st.measurements.iter().any(|x| x.id == MeasId(8)));
        assert!(!st.measurements.iter().any(|x| x.id == MeasId(7)));
        // Gap: 13 arrives, 12 missing.
        assert_eq!(m.on_event(meas_ev(13, 9), t0), Some(Need::Since(Rev(11))));
        assert_eq!(m.on_since(vec![meas_ev(12, 10), meas_ev(13, 9)], t0), None);
        assert_eq!(m.view().rev, Rev(13));
    }

    #[test]
    fn missed_final_patch_and_restart() {
        let t0 = Instant::now();
        let mut m = Mirror::new(true);
        let (s, k) = ka(1, 5);
        assert_eq!(m.on_ka(&s, k, t0, 0), Some(Need::Snapshot));
        assert_eq!(m.on_snapshot(snap(1, 5), t0), None);
        let (s, k) = ka(1, 6);
        assert_eq!(m.on_ka(&s, k, t0, 0), None);
        let t1 = t0 + Duration::from_millis(1100);
        let (s, k) = ka(1, 6);
        assert_eq!(m.on_ka(&s, k, t1, 0), Some(Need::Since(Rev(5))));
        // Restart: new incarnation drops the state.
        let (s, k) = ka(2, 1);
        assert_eq!(m.on_ka(&s, k, t1, 0), Some(Need::Rehello));
        assert!(m.view().state.is_none());
        assert_eq!(m.view().incarnation_changes, 1);
        assert_eq!(m.on_resync_required(t1), Some(Need::Snapshot));
    }

    #[test]
    fn clock_takes_least_delayed_sample() {
        let t0 = Instant::now();
        let mut c = ClockEstimator::default();
        c.add(t0, 1_000, 900); // delay-free: θ = 100
        c.add(t0, 2_000, 1_950); // delayed by 50
        assert_eq!(c.offset_ns(), Some(100));
        c.add(t0 + Duration::from_secs(11), 3_000, 2_970);
        assert_eq!(c.offset_ns(), Some(30));
    }
}
