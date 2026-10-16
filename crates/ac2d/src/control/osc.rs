//! OSC cues: the state of every cued limit of the SPL meters, sent to the rig's OSC target.
//!
//! The table is reconciled against the state after every message the control thread
//! handles, so every path that ends a meter's judging (a closed session, a lost device, a
//! stopped or deleted measurement, a removed cue) reaches the lights without its own hook:
//! - a meter whose job is not running is `fault` on every cue: the rig cannot judge;
//! - a running meter's subjects follow the job's latest report (`not_calibrated` is
//!   `fault`, `no_limit` is `ok`);
//! - the meter cue takes the worst state of all its subjects;
//! - a cue that goes away (measurement deleted, cue removed, daemon stopping cleanly) sends
//!   its `ok` once, so the light it drove does not stay red.
//!
//! A state change is sent at once; every [`REPEAT`] the current state of every cue is sent
//! again, so a lost datagram heals within a second and a receiver can treat silence as a
//! fault.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use ac2_proto::event::Change;
use ac2_proto::model::{
    CueState, CueSubject, LeqJudgement, MeasKind, OscCue, OscMessage, OscStatus, PeakQuantity,
    RigCue, SplConfig,
};
use ac2_proto::units::{MeasId, Rev};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError, ReplyBody};

use super::Control;
use crate::osc::OscSender;
use crate::rig::RigSettings;
use crate::util::{perr, perr_detail};

/// How often the current states are sent again.
pub(crate) const REPEAT: Duration = Duration::from_secs(1);

/// A cued meter reports every second; one silent this long has stopped judging (a stalled
/// device or a job that ended) even though its job is still listed, so it is `fault`.
pub(crate) const STALE: Duration = Duration::from_secs(3);

/// One subject's judgement in a job's report, with the level it was judged on (dB SPL;
/// `None` while uncalibrated or nothing is measured).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Judged {
    pub(crate) subject: CueSubject,
    pub(crate) judgement: LeqJudgement,
    pub(crate) level: Option<f64>,
}

impl Judged {
    pub(crate) fn new(subject: CueSubject, judgement: LeqJudgement, level: f64) -> Self {
        Self {
            subject,
            judgement,
            level: level.is_finite().then_some(level),
        }
    }
}

/// Whether any limit of `cfg` (or the meter) has a cue.
pub(crate) fn has_cue(cfg: &SplConfig) -> bool {
    !cues_of(cfg).is_empty()
}

/// Every cue of `cfg` with its subject, the meter's first.
fn cues_of(cfg: &SplConfig) -> Vec<(CueSubject, &OscCue)> {
    let mut out = Vec::new();
    if let Some(c) = &cfg.cue {
        out.push((CueSubject::Meter, &**c));
    }
    for w in &cfg.leq.windows {
        if let Some(c) = &w.cue {
            out.push((
                CueSubject::Window {
                    duration: w.duration,
                    weighting: w.weighting,
                },
                &**c,
            ));
        }
    }
    for q in PeakQuantity::ALL {
        if let Some(c) = cfg.leq.peaks.get(q).and_then(|p| p.cue.as_deref()) {
            out.push((CueSubject::Peak { quantity: q }, c));
        }
    }
    if let Some(b) = &cfg.bands {
        for w in &b.windows {
            if let Some(c) = &w.cue {
                out.push((
                    CueSubject::Band {
                        duration: w.duration,
                        weighting: w.weighting,
                        nominal: w.band,
                    },
                    &**c,
                ));
            }
        }
        if let Some(c) = b.predicted.as_ref().and_then(|p| p.cue.as_deref()) {
            out.push((CueSubject::Predicted, c));
        }
    }
    out
}

fn state_of(j: LeqJudgement) -> CueState {
    match j {
        LeqJudgement::NoLimit | LeqJudgement::Ok => CueState::Ok,
        LeqJudgement::Near => CueState::Near,
        LeqJudgement::Over => CueState::Over,
        LeqJudgement::NotCalibrated => CueState::Fault,
    }
}

/// Severity for the meter cue: over > fault > near > ok. A limit judged over is a fact
/// about the sound, while a fault is only the absence of a judgement: a meter with one limit
/// over and another that cannot be judged must still light "over", or the one thing the
/// light is for would be masked by a configuration gap.
fn severity(s: CueState) -> u8 {
    match s {
        CueState::Ok => 0,
        CueState::Near => 1,
        CueState::Fault => 2,
        CueState::Over => 3,
    }
}

/// What a cued subject should be now; `state: None` while a running meter has not yet
/// reported it (the entry, if any, keeps its state).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Desired {
    pub(crate) meas: MeasId,
    pub(crate) subject: CueSubject,
    pub(crate) cue: OscCue,
    pub(crate) state: Option<CueState>,
    pub(crate) level: Option<f64>,
}

/// The cued subjects of meter `meas`: `live` while its job runs, judged by `report`.
pub(crate) fn desired(
    meas: MeasId,
    cfg: &SplConfig,
    live: bool,
    report: Option<&[Judged]>,
) -> Vec<Desired> {
    cues_of(cfg)
        .into_iter()
        .map(|(subject, cue)| {
            let (state, level) = match (live, report) {
                (false, _) => (Some(CueState::Fault), None),
                (true, None) => (None, None),
                (true, Some(r)) if subject == CueSubject::Meter => r
                    .iter()
                    .map(|j| (state_of(j.judgement), j.level))
                    .max_by_key(|(s, _)| severity(*s))
                    .map_or((Some(CueState::Ok), None), |(s, l)| (Some(s), l)),
                (true, Some(r)) => r
                    .iter()
                    .find(|j| j.subject == subject)
                    .map_or((None, None), |j| (Some(state_of(j.judgement)), j.level)),
            };
            Desired {
                meas,
                subject,
                cue: cue.clone(),
                state,
                level,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    meas: MeasId,
    subject: CueSubject,
    cue: OscCue,
    state: CueState,
    level: Option<f64>,
}

/// One message to send: what, and the level to append.
pub(crate) type Send = (OscMessage, Option<f32>);

fn send_of(cue: &OscCue, state: CueState, level: Option<f64>) -> Option<Send> {
    let level = match state {
        // A fault has no level to speak of.
        CueState::Fault => None,
        _ => level.map(|l| l as f32),
    };
    cue.get(state).map(|m| (m.clone(), level))
}

/// The cued subjects' states, the latest reports and the sender.
#[derive(Debug, Default)]
pub(crate) struct Cues {
    entries: Vec<Entry>,
    /// Each running meter's latest report.
    reports: HashMap<MeasId, Vec<Judged>>,
    /// Each running cued meter's latest sign of life: its latest report, or when it was
    /// first seen running with a cue.
    alive: HashMap<MeasId, Instant>,
    sender: OscSender,
    next_repeat: Option<Instant>,
}

impl Cues {
    /// Takes `desired` (every cued subject of every meter): the messages to send for the
    /// changes, in order. A subject no longer desired sends its `ok` and is dropped.
    pub(crate) fn apply(&mut self, desired: Vec<Desired>) -> Vec<Send> {
        let mut out = Vec::new();
        let mut kept = Vec::with_capacity(desired.len());
        for e in std::mem::take(&mut self.entries) {
            if desired
                .iter()
                .any(|d| d.meas == e.meas && d.subject == e.subject)
            {
                kept.push(e);
            } else {
                out.extend(send_of(&e.cue, CueState::Ok, None));
            }
        }
        for d in desired {
            let old = kept
                .iter()
                .position(|e| e.meas == d.meas && e.subject == d.subject)
                .map(|i| kept.remove(i));
            let Some(state) = d.state.or(old.as_ref().map(|e| e.state)) else {
                continue;
            };
            let level = if d.state.is_some() {
                d.level
            } else {
                old.as_ref().and_then(|e| e.level)
            };
            let changed = old
                .as_ref()
                .is_none_or(|e| e.state != state || e.cue != d.cue);
            if changed {
                out.extend(send_of(&d.cue, state, level));
            }
            self.entries.push(Entry {
                meas: d.meas,
                subject: d.subject,
                cue: d.cue,
                state,
                level,
            });
        }
        out
    }

    /// Every cue's current message (the repeat).
    pub(crate) fn current(&self) -> Vec<Send> {
        self.entries
            .iter()
            .filter_map(|e| send_of(&e.cue, e.state, e.level))
            .collect()
    }

    /// Drops every cue: their `ok` messages.
    pub(crate) fn clear(&mut self) -> Vec<Send> {
        std::mem::take(&mut self.entries)
            .into_iter()
            .filter_map(|e| send_of(&e.cue, CueState::Ok, None))
            .collect()
    }

    /// The states, as the rig entity lists them.
    pub(crate) fn rig_cues(&self) -> Vec<RigCue> {
        self.entries
            .iter()
            .map(|e| RigCue {
                meas: e.meas,
                subject: e.subject,
                state: e.state,
            })
            .collect()
    }

    /// Keeps a running meter's report.
    pub(crate) fn reported(&mut self, meas: MeasId, judged: Vec<Judged>) {
        self.reports.insert(meas, judged);
        self.alive.insert(meas, Instant::now());
    }

    fn send_all(&mut self, target: Option<SocketAddr>, sends: Vec<Send>) {
        let Some(t) = target else {
            return;
        };
        for (m, level) in sends {
            // Counted and kept in the status; nothing else to do with a lost datagram.
            let _ = self.sender.send(t, &m, level);
        }
    }

    pub(crate) fn status(&self) -> &OscStatus {
        self.sender.status()
    }
}

impl Control {
    /// Reconciles the cues with the state and sends what changed.
    pub(super) fn osc_sync(&mut self) {
        let now = Instant::now();
        let mut want = Vec::new();
        for m in &self.store.state().measurements {
            let MeasKind::Spl { config } = &m.config.kind else {
                continue;
            };
            let cued = has_cue(config);
            let mut live = self.session.is_some() && self.jobs.contains_key(&m.id);
            if live && cued {
                let seen = *self.osc.alive.entry(m.id).or_insert(now);
                live = now.saturating_duration_since(seen) < STALE;
            } else {
                self.osc.alive.remove(&m.id);
            }
            if !live {
                self.osc.reports.remove(&m.id);
            }
            if cued {
                want.extend(desired(
                    m.id,
                    config,
                    live,
                    self.osc.reports.get(&m.id).map(Vec::as_slice),
                ));
            }
        }
        let measurements = &self.store.state().measurements;
        self.osc
            .reports
            .retain(|id, _| measurements.iter().any(|m| m.id == *id));
        self.osc
            .alive
            .retain(|id, _| measurements.iter().any(|m| m.id == *id));
        let sends = self.osc.apply(want);
        let target = self.store.state().rig.osc_target;
        self.osc.send_all(target, sends);
        if self.osc.entries.is_empty() {
            self.osc.next_repeat = None;
        } else if self.osc.next_repeat.is_none() {
            self.osc.next_repeat = Some(Instant::now() + REPEAT);
        }
        let cues = self.osc.rig_cues();
        if cues != self.store.state().rig.cues {
            let mut rig = self.store.state().rig.clone();
            rig.cues = cues;
            rig.osc = self.osc.status().clone();
            self.commit(Change::Rig(rig));
        }
    }

    /// The repeat, when due; the sending status is committed here (at most once a second)
    /// when it changed.
    pub(super) fn osc_tick(&mut self, now: Instant) {
        if self.osc.next_repeat.is_some_and(|t| t <= now) {
            let sends = self.osc.current();
            let target = self.store.state().rig.osc_target;
            self.osc.send_all(target, sends);
            self.osc.next_repeat = Some(now + REPEAT);
            self.commit_osc_status();
        }
    }

    fn commit_osc_status(&mut self) {
        if *self.osc.status() != self.store.state().rig.osc {
            let mut rig = self.store.state().rig.clone();
            rig.osc = self.osc.status().clone();
            self.commit(Change::Rig(rig));
        }
    }

    /// When the control loop must wake for the repeat.
    pub(super) fn osc_wake(&self) -> Option<Instant> {
        self.osc.next_repeat
    }

    /// Clean shutdown: every cue sends its `ok` once.
    pub(super) fn osc_shutdown(&mut self) {
        let sends = self.osc.clear();
        let target = self.store.state().rig.osc_target;
        self.osc.send_all(target, sends);
    }

    /// `osc.target`.
    pub(super) fn osc_target(
        &mut self,
        base: Rev,
        addr: Option<SocketAddr>,
    ) -> Result<ReplyBody, ProtoError> {
        self.rig_base(base)?;
        if let Some(a) = addr
            && (a.ip().is_unspecified() || a.port() == 0)
        {
            return Err(perr(
                ErrorCode::Invalid,
                format!("{a} is not an address to send to: give a host's IP and a port"),
            ));
        }
        let mut rig = self.store.state().rig.clone();
        if rig.osc_target == addr {
            return Ok(ReplyBody::Ack {
                rev: self.store.rev(),
            });
        }
        // Kept first: a target that would come back different after a restart would send
        // the lights' cues somewhere the operator did not choose.
        self.rig.persist(&RigSettings {
            ceiling_dbfs: self.ceiling_set,
            outputs: self.store.state().outputs.clone(),
            osc_target: addr,
        })?;
        tracing::info!(
            "OSC target {}",
            addr.map_or_else(|| "off".to_owned(), |a| a.to_string())
        );
        rig.osc_target = addr;
        rig.rev = Rev(self.store.rev().0 + 1);
        let rev = self.commit(Change::Rig(rig));
        // The new receiver learns the current states now, not at the next repeat.
        let sends = self.osc.current();
        self.osc.send_all(addr, sends);
        self.commit_osc_status();
        Ok(ReplyBody::Ack { rev })
    }

    /// `osc.test`.
    pub(super) fn osc_test(&mut self, msg: &OscMessage) -> Result<ReplyBody, ProtoError> {
        let Some(target) = self.store.state().rig.osc_target else {
            return Err(perr(
                ErrorCode::Invalid,
                "no OSC target is set (osc.target)",
            ));
        };
        let r = self.osc.sender.send(target, msg, Some(0.0));
        self.commit_osc_status();
        r.map_err(|e| perr(ErrorCode::Internal, e))?;
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }

    /// `conflict` unless `base` is the rig's rev.
    fn rig_base(&self, base: Rev) -> Result<(), ProtoError> {
        let rev = self.store.state().rig.rev;
        if base == rev {
            return Ok(());
        }
        Err(perr_detail(
            ErrorCode::Conflict,
            format!(
                "the rig settings changed since (now at rev {}); re-read them",
                rev.0
            ),
            ErrorDetail::Conflict { rev },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{OscPath, Weighting};
    use ac2_proto::units::Seconds;

    fn msg(p: &str) -> OscMessage {
        OscMessage {
            path: OscPath::new(p).expect("test value"),
            args: vec![],
            append_level: true,
        }
    }

    fn cue() -> OscCue {
        OscCue {
            ok: Some(msg("/ok")),
            near: Some(msg("/near")),
            over: Some(msg("/over")),
            fault: Some(msg("/fault")),
        }
    }

    fn win() -> CueSubject {
        CueSubject::Window {
            duration: Seconds(60.0),
            weighting: Weighting::A,
        }
    }

    fn peak() -> CueSubject {
        CueSubject::Peak {
            quantity: PeakQuantity::LcPeak,
        }
    }

    fn want(subject: CueSubject, state: Option<CueState>) -> Desired {
        Desired {
            meas: MeasId(1),
            subject,
            cue: cue(),
            state,
            level: Some(90.0),
        }
    }

    fn paths(s: &[Send]) -> Vec<&str> {
        s.iter().map(|(m, _)| m.path.as_str()).collect()
    }

    #[test]
    fn sends_each_change_once() {
        let mut c = Cues::default();
        let mut seen = Vec::new();
        for s in [
            CueState::Ok,
            CueState::Ok,
            CueState::Near,
            CueState::Over,
            CueState::Over,
            CueState::Ok,
        ] {
            seen.extend(c.apply(vec![want(win(), Some(s))]));
        }
        assert_eq!(paths(&seen), ["/ok", "/near", "/over", "/ok"]);
        assert_eq!(seen[2].1, Some(90.0));
        assert_eq!(paths(&c.current()), ["/ok"]);
    }

    #[test]
    fn judgements_map_to_states() {
        assert_eq!(state_of(LeqJudgement::NoLimit), CueState::Ok);
        assert_eq!(state_of(LeqJudgement::NotCalibrated), CueState::Fault);
        assert_eq!(state_of(LeqJudgement::Near), CueState::Near);
        assert_eq!(state_of(LeqJudgement::Over), CueState::Over);
    }

    #[test]
    fn unreported_keeps_the_state_and_new_waits() {
        let mut c = Cues::default();
        assert!(c.apply(vec![want(win(), None)]).is_empty());
        assert!(c.rig_cues().is_empty());
        c.apply(vec![want(win(), Some(CueState::Fault))]);
        assert!(c.apply(vec![want(win(), None)]).is_empty());
        assert_eq!(c.rig_cues()[0].state, CueState::Fault);
    }

    #[test]
    fn removal_sends_ok_once() {
        let mut c = Cues::default();
        c.apply(vec![
            want(win(), Some(CueState::Over)),
            want(peak(), Some(CueState::Near)),
        ]);
        let s = c.apply(vec![want(peak(), Some(CueState::Near))]);
        assert_eq!(paths(&s), ["/ok"]);
        assert_eq!(s[0].1, None);
        assert!(c.apply(vec![want(peak(), Some(CueState::Near))]).is_empty());
        assert_eq!(paths(&c.clear()), ["/ok"]);
        assert!(c.current().is_empty());
    }

    #[test]
    fn changed_cue_is_sent_again() {
        let mut c = Cues::default();
        c.apply(vec![want(win(), Some(CueState::Over))]);
        let mut d = want(win(), Some(CueState::Over));
        d.cue.over = Some(msg("/red"));
        assert_eq!(paths(&c.apply(vec![d])), ["/red"]);
    }

    #[test]
    fn fault_appends_no_level() {
        let mut c = Cues::default();
        let s = c.apply(vec![want(win(), Some(CueState::Fault))]);
        assert_eq!(s[0].1, None);
    }

    fn spl_with_cues() -> SplConfig {
        let mut cfg = ac2_proto::samples::spl_config();
        cfg.bands = None;
        cfg.cue = Some(Box::new(cue()));
        cfg.leq.windows[0].cue = Some(Box::new(cue()));
        cfg
    }

    fn judged(subject: CueSubject, j: LeqJudgement, level: f64) -> Judged {
        Judged::new(subject, j, level)
    }

    #[test]
    fn meter_is_the_worst_and_over_beats_fault() {
        let cfg = spl_with_cues();
        let w = cfg.leq.windows[0].clone();
        let ws = CueSubject::Window {
            duration: w.duration,
            weighting: w.weighting,
        };
        let state = |r: &[Judged]| {
            desired(MeasId(1), &cfg, true, Some(r))
                .into_iter()
                .map(|d| (d.subject, d.state, d.level))
                .collect::<Vec<_>>()
        };
        let r = [
            judged(ws, LeqJudgement::Near, 95.0),
            judged(peak(), LeqJudgement::NoLimit, f64::NAN),
        ];
        assert_eq!(
            state(&r),
            [
                (CueSubject::Meter, Some(CueState::Near), Some(95.0)),
                (ws, Some(CueState::Near), Some(95.0)),
            ]
        );
        let r = [
            judged(ws, LeqJudgement::Over, 101.0),
            judged(peak(), LeqJudgement::NotCalibrated, f64::NAN),
        ];
        assert_eq!(
            state(&r)[0],
            (CueSubject::Meter, Some(CueState::Over), Some(101.0))
        );
        let r = [
            judged(ws, LeqJudgement::Near, 95.0),
            judged(peak(), LeqJudgement::NotCalibrated, f64::NAN),
        ];
        assert_eq!(state(&r)[0].1, Some(CueState::Fault));
        // No report yet: nothing to say; not running: fault on every cue.
        assert!(
            desired(MeasId(1), &cfg, true, None)
                .iter()
                .all(|d| d.state.is_none())
        );
        assert!(
            desired(MeasId(1), &cfg, false, Some(&r))
                .iter()
                .all(|d| d.state == Some(CueState::Fault))
        );
    }
}
