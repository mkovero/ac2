//! SPL logs and Leq windows on the control thread: one log per SPL measurement (kept
//! across its job's restarts), the `spl_log` entity with each window's state and the
//! alarms, `spl.log_get`, `spl.history_get`, `spl.log_new` (`docs/design/leq.md`). The
//! autosave's thread keeps the logs on disk; this thread only tells it which logs exist.

use std::sync::{Arc, Mutex};

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{
    AlarmSubject, BAND_COUNT, BandLevelSource, BandTransferSet, LeqAlarm, LeqAlarmKind, LeqConfig,
    LeqJudgement, MeasKind, Measurement, PeakQuantity, SplHistory, SplLog, SplLogWhich, Weighting,
};
use ac2_proto::units::{ClientId, MeasId, Rev, WallNs};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};
use ac2_traces::session::{SavedSplLog, SplLogOnDisk};
use ac2_traces::spl_log::SplLogInfo;

use super::Control;
use crate::autosave::{LogSpec, Resume};
use crate::jobs::spl::LeqSetup;
use crate::leq_history;
use crate::leq_log::{self, LeqLog};
use crate::util::{perr, wall_ns};

/// `LAeq 30 min`, `LCpeak`, for the daemon's own log.
fn subject_name(s: &AlarmSubject) -> String {
    match s {
        AlarmSubject::Window {
            duration,
            weighting,
        } => {
            let w = match weighting {
                Weighting::A => "A",
                Weighting::C => "C",
                Weighting::Z => "Z",
            };
            let secs = duration.0;
            let len = if secs >= 60.0 && secs % 60.0 == 0.0 {
                format!("{} min", secs / 60.0)
            } else {
                format!("{secs} s")
            };
            format!("L{w}eq {len}")
        }
        AlarmSubject::Peak {
            quantity: PeakQuantity::LcPeak,
        } => "LCpeak".into(),
        AlarmSubject::Peak {
            quantity: PeakQuantity::LafMax,
        } => "LAFmax".into(),
        AlarmSubject::Band { nominal } => format!("{} Hz band Leq", nominal.0),
        AlarmSubject::Predicted => "predicted dwelling LAeq".into(),
    }
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
        let calibrated = self.input_calibrated(config.input);
        let now = WallNs(wall_ns());
        let windows = leq_log::window_states(
            &config.leq,
            old,
            prev.as_ref().map_or(&[][..], |p| &p.windows),
            calibrated,
            now,
        );
        let peaks = leq_log::peak_states(
            &config.leq,
            old,
            prev.as_ref().map(|p| p.peaks),
            calibrated,
            now,
        );
        self.commit_spl_log(SplLog {
            meas: m.id,
            started_at,
            windows,
            peaks,
            alarms: prev.map(|p| p.alarms).unwrap_or_default(),
        });
        // A new log is a change to the autosave (its file is named in the manifest).
        self.autosave_changed();
    }

    /// Loads `rows` as the log of SPL measurement `id` (a loaded session's).
    pub(super) fn set_spl_log(&mut self, id: MeasId, log: LeqLog) {
        self.spl_logs.insert(id, Arc::new(Mutex::new(log)));
        self.autosave_changed();
    }

    /// Forgets a measurement's logs and its entity.
    pub(super) fn drop_spl_log(&mut self, id: MeasId) {
        let had = self.spl_logs.remove(&id).is_some();
        self.spl_prev_logs.remove(&id);
        if self.spl_log_entity(id).is_some() {
            self.commit(Change::SplLog(Patch::Deleted(id)));
        }
        if had {
            self.autosave_changed();
        }
    }

    /// What a starting SPL job needs: the log and the judgements the entity holds.
    pub(super) fn leq_setup(&mut self, id: MeasId) -> LeqSetup {
        let log = match self.spl_logs.get(&id) {
            Some(l) => l.clone(),
            None => {
                let l: crate::leq_log::SharedLog = Arc::new(Mutex::new(LeqLog::default()));
                self.spl_logs.insert(id, l.clone());
                self.autosave_changed();
                l
            }
        };
        let entity = self.spl_log_entity(id);
        LeqSetup {
            log,
            to_control: self.s.to_self.clone(),
            judgements: entity
                .map(|l| l.windows.iter().map(|w| w.judgement).collect())
                .unwrap_or_default(),
            peak_judgements: PeakQuantity::ALL
                .map(|q| entity.map_or(LeqJudgement::NoLimit, |l| l.peaks.get(q).judgement)),
            local: self.s.local_clock,
        }
    }

    /// A job reported its windows' and peak limits' judgements after a second (or a log's
    /// first row).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn leq_reported(
        &mut self,
        meas: MeasId,
        epoch: u64,
        config_rev: Rev,
        at: WallNs,
        judgements: &[LeqJudgement],
        peak_judgements: [LeqJudgement; 2],
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
        for (q, j) in PeakQuantity::ALL.into_iter().zip(peak_judgements) {
            let s = l.peaks.get_mut(q);
            if s.judgement != j {
                s.judgement = j;
                s.since = at;
            }
        }
        for a in &alarms {
            let corrected = match a.position {
                Some(p) => format!(" (corrected {:+.1} dB)", p.0),
                None => String::new(),
            };
            match a.kind {
                LeqAlarmKind::Over => tracing::warn!(
                    "SPL meter {meas} ({name}): {} over its limit: {:.1} dB > {:.1} dB{corrected}",
                    subject_name(&a.subject),
                    a.level.0,
                    a.limit.0
                ),
                LeqAlarmKind::Recovered => tracing::info!(
                    "SPL meter {meas} ({name}): {} back within its limit: {:.1} dB ≤ {:.1} dB{corrected}",
                    subject_name(&a.subject),
                    a.level.0,
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

    /// `spl.history_get`: the meter's windows second by second over the newest `seconds`
    /// of its current log, replayed from rows copied out of it (the job goes on logging).
    pub(super) fn spl_history_get(
        &self,
        meas: MeasId,
        seconds: u32,
    ) -> Result<ReplyBody, ProtoError> {
        let m = self.spl_meter(meas)?;
        let MeasKind::Spl { config } = &m.config.kind else {
            return Err(perr(ErrorCode::Internal, "SPL meter without its config"));
        };
        let seconds = seconds.min(SplHistory::MAX_SECONDS);
        let (rows, from_log_start) = match self.spl_logs.get(&meas) {
            Some(l) => leq_log::lock(l).tail(leq_history::rows_needed(&config.leq, seconds)),
            None => (Vec::new(), true),
        };
        Ok(ReplyBody::SplHistory(Box::new(leq_history::history(
            meas,
            &config.leq,
            &rows,
            from_log_start,
            seconds,
        ))))
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
        let calibrated = self.input_calibrated(config.input);
        let now = WallNs(wall_ns());
        let windows = leq_log::window_states(&config.leq, None, &[], calibrated, now);
        let peaks = leq_log::peak_states(&config.leq, None, None, calibrated, now);
        self.commit_spl_log(SplLog {
            meas,
            started_at: None,
            windows,
            peaks,
            alarms: Vec::new(),
        });
        self.autosave_changed();
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }

    /// Which log each SPL meter has, for the autosave's change check: a log's growth is
    /// not a change (the autosave appends it), another log is.
    pub(super) fn spl_log_ids(&self) -> Vec<(MeasId, u64)> {
        let mut v: Vec<(MeasId, u64)> = self
            .spl_logs
            .iter()
            .map(|(id, l)| (*id, leq_log::lock(l).id()))
            .collect();
        v.sort_by_key(|(id, _)| *id);
        v
    }

    /// Each SPL meter's log with what its file header names, by measurement.
    fn spl_log_infos(&self) -> Vec<(SplLogInfo, &crate::leq_log::SharedLog)> {
        let st = self.store.state();
        let mut out: Vec<_> = self
            .spl_logs
            .iter()
            .filter_map(|(id, log)| {
                let (name, config) = self.spl_config(*id)?;
                let mic = st
                    .inputs
                    .iter()
                    .find(|i| i.channel == config.input)
                    .and_then(|i| i.mic.clone());
                Some((
                    SplLogInfo {
                        meas: *id,
                        name,
                        input: config.input,
                        mic,
                    },
                    log,
                ))
            })
            .collect();
        out.sort_by_key(|(i, _)| i.meas);
        out
    }

    /// Every SPL meter's log as a session saves it.
    pub(super) fn saved_spl_logs(&self) -> Vec<SavedSplLog> {
        self.spl_log_infos()
            .into_iter()
            .map(|(info, log)| {
                let l = leq_log::lock(log);
                SavedSplLog {
                    info,
                    rows: l.rows(),
                    bands: l.band_rows(),
                }
            })
            .collect()
    }

    /// Every SPL meter's log for the autosave to keep on disk; `resume` says where the
    /// files of restored logs stand.
    pub(super) fn spl_log_specs(&self, resume: &[SplLogOnDisk]) -> Vec<LogSpec> {
        self.spl_log_infos()
            .into_iter()
            .map(|(info, log)| {
                let on_disk = resume.iter().find(|r| r.meas == info.meas);
                LogSpec {
                    meas: info.meas,
                    log: log.clone(),
                    resume: on_disk.map(|r| Resume {
                        file: r.file.clone(),
                        rows: r.rows,
                        complete_len: r.complete_len,
                    }),
                    resume_bands: on_disk.and_then(|r| r.bands.as_ref()).map(|b| Resume {
                        file: b.file.clone(),
                        rows: b.rows,
                        complete_len: b.complete_len,
                    }),
                    info,
                }
            })
            .collect()
    }
}

impl Control {
    /// Band levels, dB SPL, per band (NaN where not measured) from `src`.
    fn band_levels(&self, src: &BandLevelSource) -> Result<[f64; BAND_COUNT], ProtoError> {
        let inv = |m: String| perr(ErrorCode::Invalid, m);
        match src {
            BandLevelSource::Levels { levels } => {
                if levels.len() != BAND_COUNT {
                    return Err(inv(format!(
                        "band levels are {BAND_COUNT} values, 20 Hz … 10 kHz"
                    )));
                }
                if levels.iter().flatten().any(|l| !l.0.is_finite()) {
                    return Err(inv("a band level must be finite".into()));
                }
                let mut out = [f64::NAN; BAND_COUNT];
                for (o, l) in out.iter_mut().zip(levels) {
                    if let Some(l) = l {
                        *o = l.0;
                    }
                }
                Ok(out)
            }
            BandLevelSource::Log { meas, from, until } => {
                if until <= from {
                    return Err(inv("the span ends before it starts".into()));
                }
                let rows = self.band_rows_in(*meas, *from, *until)?;
                ac2_traces::band_log::span_average(&rows)
                    .levels()
                    .map_err(|g| inv(format!("SPL meter {meas}: {g}")))
            }
        }
    }

    /// The band seconds of SPL meter `meas`'s current log starting in `[from, until)`.
    fn band_rows_in(
        &self,
        meas: MeasId,
        from: WallNs,
        until: WallNs,
    ) -> Result<Vec<ac2_traces::band_log::BandLogRow>, ProtoError> {
        let m = self.spl_meter(meas)?;
        let banded = matches!(&m.config.kind, MeasKind::Spl { config } if config.bands.is_some());
        let name = m.config.name.clone();
        let rows = self
            .spl_logs
            .get(&meas)
            .map(|l| leq_log::lock(l).band_rows_in(from, until))
            .unwrap_or_default();
        // A band meter turned off keeps its log: only an empty span of a meter without one
        // says how to get a log.
        if rows.is_empty() && !banded {
            return Err(perr(
                ErrorCode::Invalid,
                format!(
                    "{name} has no band meter, so it logs no band levels: turn it on (Leq \
                     settings, or `ac2 spl bands set --preset finland-545-lf`), calibrate it, \
                     then measure the span again"
                ),
            ));
        }
        Ok(rows)
    }

    /// `spl.band_log_get`: a span of the meter's band log, averaged as a transfer takes it,
    /// with every `step`-th second.
    pub(super) fn spl_band_log_get(
        &self,
        meas: MeasId,
        from: WallNs,
        until: WallNs,
        step: Option<u32>,
    ) -> Result<ReplyBody, ProtoError> {
        let rows = self.band_rows_in(meas, from, until)?;
        ac2_traces::band_log::span_reply(meas, from, until, step, &rows)
            .map(|r| ReplyBody::SplBandLog(Box::new(r)))
            .map_err(|m| perr(ErrorCode::Invalid, m))
    }

    /// `spl.band_transfer`: the transfer computed and stored in the band meter's
    /// configuration, applied as `meas.update` applies any change to it.
    pub(super) fn spl_band_transfer(
        &mut self,
        client: &ClientId,
        meas: MeasId,
        foh: &BandLevelSource,
        dwelling: &BandLevelSource,
        background: Option<&BandLevelSource>,
    ) -> Result<ReplyBody, ProtoError> {
        let m = self.spl_meter(meas)?.clone();
        let MeasKind::Spl { config } = &m.config.kind else {
            return Err(perr(
                ErrorCode::Invalid,
                format!("{meas} is not an SPL meter"),
            ));
        };
        if config.bands.is_none() {
            return Err(perr(
                ErrorCode::Invalid,
                format!("SPL meter {meas} has no band meter: enable it first"),
            ));
        }
        let name = |id: MeasId| {
            self.spl_meter(id)
                .map_or_else(|_| format!("SPL meter {id}"), |m| m.config.name.clone())
        };
        if let Some(e) = ac2_proto::model::overlapping_spans(foh, dwelling, background, name) {
            return Err(perr(ErrorCode::Invalid, e));
        }
        let foh = self.band_levels(foh)?;
        let dwelling = self.band_levels(dwelling)?;
        let background = background.map(|b| self.band_levels(b)).transpose()?;
        let t = ac2_core::band_leq::Transfer::measure(&foh, &dwelling, background.as_ref());
        let set = BandTransferSet {
            measured_at: WallNs(wall_ns()),
            origin: ac2_proto::model::TransferOrigin::Measured,
            bands: t.bands().map(crate::conv::band_transfer_band),
        };
        let mut config = m.config.clone();
        if let MeasKind::Spl { config: c } = &mut config.kind
            && let Some(b) = &mut c.bands
        {
            b.transfer = Some(set);
        }
        tracing::info!(
            "SPL meter {meas}: FOH → dwelling transfer stored ({} of {BAND_COUNT} bands measured)",
            set.bands
                .iter()
                .filter(|b| !matches!(b, ac2_proto::model::BandTransferBand::Missing))
                .count()
        );
        self.execute(client, ac2_proto::Command::MeasUpdate { meas, config })
    }
}
