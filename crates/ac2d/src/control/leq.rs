//! SPL logs and Leq windows on the control thread: one log per SPL measurement (kept
//! across its job's restarts), the `spl_log` entity with each window's state and the
//! alarms, `spl.log_get`, `spl.log_new` (`docs/design/leq.md`).

use std::sync::{Arc, Mutex};

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    LeqAlarm, LeqAlarmKind, LeqConfig, LeqJudgement, LeqWindowState, MeasKind, Measurement, SplLog,
    SplLogWhich,
};
use ac2_proto::units::{MeasId, Rev, WallNs};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};
use ac2_traces::session::SavedSplLog;
use ac2_traces::spl_log::SplLogInfo;

use super::Control;
use crate::jobs::spl::LeqSetup;
use crate::leq_log::{self, LeqLog};
use crate::util::{perr, wall_ns};

/// `LAeq 30 min`, for the daemon's own log.
fn window_name(s: &LeqWindowState) -> String {
    let w = match s.weighting {
        ac2_proto::model::Weighting::A => "A",
        ac2_proto::model::Weighting::C => "C",
        ac2_proto::model::Weighting::Z => "Z",
    };
    let secs = s.duration.0;
    let len = if secs >= 60.0 && secs % 60.0 == 0.0 {
        format!("{} min", secs / 60.0)
    } else {
        format!("{secs} s")
    };
    format!("L{w}eq {len}")
}

impl Control {
    fn spl_config(&self, id: MeasId) -> Option<(String, ac2_proto::model::SplConfig)> {
        self.store
            .state()
            .measurements
            .iter()
            .find(|m| m.id == id)
            .and_then(|m| match &m.config.kind {
                MeasKind::Spl { config } => Some((m.config.name.clone(), config.clone())),
                _ => None,
            })
    }

    /// Whether `input` of the open session has a sensitivity calibration.
    fn input_calibrated(&self, input: u16) -> bool {
        self.session
            .as_ref()
            .is_some_and(|rt| self.input_cal(rt, input).sensitivity.is_some())
    }

    fn spl_log_entity(&self, id: MeasId) -> Option<&SplLog> {
        self.store.state().spl_logs.iter().find(|l| l.meas == id)
    }

    fn commit_spl_log(&mut self, l: SplLog) {
        if self.spl_log_entity(l.meas) != Some(&l) {
            self.commit(Change::SplLog(Patch::Set(l)));
        }
    }

    /// Gives an SPL measurement its log (an empty one, or `rows` of a loaded session) when
    /// it has none, and its `spl_log` entity the windows of its configuration. `old` is
    /// the configuration before an update: windows configured as before keep their state.
    pub(super) fn ensure_spl_log(&mut self, m: &Measurement, old: Option<&LeqConfig>) {
        let MeasKind::Spl { config } = &m.config.kind else {
            self.drop_spl_log(m.id);
            return;
        };
        let log = self
            .spl_logs
            .entry(m.id)
            .or_insert_with(|| Arc::new(Mutex::new(LeqLog::default())))
            .clone();
        let started_at = leq_log::lock(&log).started_at();
        let prev = self.spl_log_entity(m.id).cloned();
        let windows = leq_log::window_states(
            &config.leq,
            old,
            prev.as_ref().map_or(&[][..], |p| &p.windows),
            self.input_calibrated(config.input),
            WallNs(wall_ns()),
        );
        self.commit_spl_log(SplLog {
            meas: m.id,
            started_at,
            windows,
            alarms: prev.map(|p| p.alarms).unwrap_or_default(),
        });
    }

    /// Loads `rows` as the log of SPL measurement `id` (a loaded session's).
    pub(super) fn set_spl_log(&mut self, id: MeasId, log: LeqLog) {
        self.spl_logs.insert(id, Arc::new(Mutex::new(log)));
    }

    /// Forgets a measurement's logs and its entity.
    pub(super) fn drop_spl_log(&mut self, id: MeasId) {
        self.spl_logs.remove(&id);
        self.spl_prev_logs.remove(&id);
        if self.spl_log_entity(id).is_some() {
            self.commit(Change::SplLog(Patch::Deleted(id)));
        }
    }

    /// What a starting SPL job needs: the log and the judgements the entity holds.
    pub(super) fn leq_setup(&mut self, id: MeasId) -> LeqSetup {
        let log = self
            .spl_logs
            .entry(id)
            .or_insert_with(|| Arc::new(Mutex::new(LeqLog::default())))
            .clone();
        LeqSetup {
            log,
            to_control: self.s.to_self.clone(),
            judgements: self
                .spl_log_entity(id)
                .map(|l| l.windows.iter().map(|w| w.judgement).collect())
                .unwrap_or_default(),
        }
    }

    /// A job reported its windows' judgements after a second (or a log's first row).
    pub(super) fn leq_reported(
        &mut self,
        meas: MeasId,
        epoch: u64,
        config_rev: Rev,
        at: WallNs,
        judgements: &[LeqJudgement],
        alarms: Vec<LeqAlarm>,
    ) {
        let current = self
            .store
            .state()
            .measurements
            .iter()
            .find(|m| m.id == meas)
            .map(|m| m.config_rev);
        // A report from a job the configuration has moved on from describes other windows;
        // one from before `spl.log_new` belongs to the ended log.
        if current != Some(config_rev)
            || self
                .spl_logs
                .get(&meas)
                .is_none_or(|l| leq_log::lock(l).epoch() != epoch)
        {
            return;
        }
        let (Some(prev), Some((name, _))) =
            (self.spl_log_entity(meas).cloned(), self.spl_config(meas))
        else {
            return;
        };
        let mut l = prev;
        for (w, &j) in l.windows.iter_mut().zip(judgements) {
            if w.judgement != j {
                w.judgement = j;
                w.since = at;
            }
        }
        for a in &alarms {
            let s = LeqWindowState {
                duration: a.duration,
                weighting: a.weighting,
                judgement: LeqJudgement::Over,
                since: a.at,
            };
            match a.kind {
                LeqAlarmKind::Over => tracing::warn!(
                    "SPL meter {meas} ({name}): {} over its limit: {:.1} dB > {:.1} dB",
                    window_name(&s),
                    a.leq.0,
                    a.limit.0
                ),
                LeqAlarmKind::Recovered => tracing::info!(
                    "SPL meter {meas} ({name}): {} back within its limit: {:.1} dB ≤ {:.1} dB",
                    window_name(&s),
                    a.leq.0,
                    a.limit.0
                ),
            }
        }
        l.alarms.extend(alarms);
        let over = l.alarms.len().saturating_sub(SplLog::MAX_ALARMS);
        l.alarms.drain(..over);
        if let Some(log) = self.spl_logs.get(&meas) {
            l.started_at = leq_log::lock(log).started_at();
        }
        self.commit_spl_log(l);
    }

    /// `invalid` unless `meas` is an SPL meter.
    fn spl_meter(&self, meas: MeasId) -> Result<&Measurement, ProtoError> {
        let m = self.meas(meas)?;
        if !matches!(m.config.kind, MeasKind::Spl { .. }) {
            return Err(perr(
                ErrorCode::Invalid,
                format!("measurement {meas} is not an SPL meter"),
            ));
        }
        Ok(m)
    }

    /// `spl.log_get`.
    pub(super) fn spl_log_get(
        &self,
        meas: MeasId,
        which: SplLogWhich,
        from: u64,
        max: u32,
    ) -> Result<ReplyBody, ProtoError> {
        self.spl_meter(meas)?;
        let page = match which {
            SplLogWhich::Current => match self.spl_logs.get(&meas) {
                Some(l) => leq_log::lock(l).page(meas, from, max),
                None => LeqLog::default().page(meas, from, max),
            },
            SplLogWhich::Previous => self
                .spl_prev_logs
                .get(&meas)
                .ok_or_else(|| {
                    perr(
                        ErrorCode::NotFound,
                        format!(
                            "SPL meter {meas} has no previous log: none was ended since the \
                             daemon started"
                        ),
                    )
                })?
                .page(meas, from, max),
        };
        Ok(ReplyBody::SplLogPage(page))
    }

    /// `spl.log_new`: the current log becomes the previous one and an empty log starts;
    /// the entity's windows go back to their initial states and the alarms are cleared. A
    /// running job sees the new log at its next second and starts its windows over.
    pub(super) fn spl_log_new(&mut self, meas: MeasId) -> Result<ReplyBody, ProtoError> {
        let m = self.spl_meter(meas)?.clone();
        let MeasKind::Spl { config } = &m.config.kind else {
            return Err(perr(ErrorCode::Internal, "SPL meter without its config"));
        };
        let log = self
            .spl_logs
            .entry(meas)
            .or_insert_with(|| Arc::new(Mutex::new(LeqLog::default())))
            .clone();
        let ended = {
            let mut l = leq_log::lock(&log);
            let next = l.next();
            std::mem::replace(&mut *l, next)
        };
        if let Some(run) = ended.run() {
            tracing::info!(
                "SPL meter {meas} ({}): new log; the one ended ran {:.0} s from {} ({} rows)",
                m.config.name,
                (run.until.0.saturating_sub(run.started_at.0)) as f64 / 1e9,
                ac2_traces::spl_log::utc_iso(run.started_at.0),
                ended.total()
            );
        }
        self.spl_prev_logs.insert(meas, ended);
        let windows = leq_log::window_states(
            &config.leq,
            None,
            &[],
            self.input_calibrated(config.input),
            WallNs(wall_ns()),
        );
        self.commit_spl_log(SplLog {
            meas,
            started_at: None,
            windows,
            alarms: Vec::new(),
        });
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }

    /// Which log and how many rows per SPL meter, for the autosave's change check.
    pub(super) fn spl_log_totals(&self) -> Vec<(MeasId, u64, u64)> {
        let mut v: Vec<(MeasId, u64, u64)> = self
            .spl_logs
            .iter()
            .map(|(id, l)| {
                let l = leq_log::lock(l);
                (*id, l.epoch(), l.total())
            })
            .collect();
        v.sort_by_key(|(id, ..)| *id);
        v
    }

    /// Every SPL meter's log as a session saves it.
    pub(super) fn saved_spl_logs(&self) -> Vec<SavedSplLog> {
        let st = self.store.state();
        let mut out: Vec<SavedSplLog> = self
            .spl_logs
            .iter()
            .filter_map(|(id, log)| {
                let (name, config) = self.spl_config(*id)?;
                let mic = st
                    .inputs
                    .iter()
                    .find(|i| i.channel == config.input)
                    .and_then(|i| i.mic.clone());
                Some(SavedSplLog {
                    info: SplLogInfo {
                        meas: *id,
                        name,
                        input: config.input,
                        mic,
                    },
                    rows: leq_log::lock(log).rows(),
                })
            })
            .collect();
        out.sort_by_key(|l| l.info.meas);
        out
    }
}
