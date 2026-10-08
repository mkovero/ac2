//! The band meter of an SPL meter (`docs/design/band-leq.md`): the unweighted, mic-curve
//! corrected signal through the 1/3-octave bank, one [`BandSecond`] per second of the
//! meter's own second grid, logged next to its SPL log, rolling windows on the bands
//! 20 … 200 Hz judged against the limits of the period in force, and the dwelling LAeq
//! predicted through a FOH → dwelling transfer.

use ac2_core::band_leq::{
    BandIntegrator, BandLimits, BandSecond, BandState, BandWindows, LF_BANDS, Period,
    PredictedSecond, Transfer, worst_band,
};
use ac2_core::leq::{Headroom, Judgement, Latch, RollingLeq, judge_window};
use ac2_core::spectrum::power_dbfs;
use ac2_proto::frame::BandLeqMeta;
use ac2_proto::model::{
    AlarmSubject, BAND_NOMINAL_HZ, BandLeqBand, BandLeqConfig, BandLimitPlace, CalStatus, LeqAlarm,
    LeqAlarmKind, LeqJudgement, LevelScale, PredictedLeq,
};
use ac2_proto::units::{Db, DbSpl, Hz, Seconds, WallNs};
use ac2_traces::band_log::BandLogRow;

use crate::config::LocalClock;
use crate::conv;
use crate::leq_log::{self, SharedLog};

const NS: u64 = 1_000_000_000;

/// A band meter's state between seconds.
pub(super) struct BandMeter {
    pub(super) cfg: BandLeqConfig,
    seconds: u32,
    horizon: u32,
    pub(super) integ: BandIntegrator,
    windows: BandWindows,
    /// Dwelling A-weighted energy predicted from each FOH second (only with a transfer).
    predicted: Option<(Transfer, RollingLeq<PredictedSecond>)>,
    /// The limits judged at the mic: the dwelling's, plus each band's attenuation with a
    /// transfer.
    limits: BandLimits,
    /// Completed seconds of the block being processed (capacity kept between blocks).
    pub(super) done: Vec<BandSecond>,
    /// Sample at which the current second started; `None` until the first block.
    pub(super) second_start: Option<u64>,
    states: [BandState; LF_BANDS],
    latches: [Latch; LF_BANDS],
    judgements: [LeqJudgement; LF_BANDS],
    predicted_latch: Latch,
    predicted_judgement: LeqJudgement,
    at_horizon: Period,
    epoch: u64,
    local: LocalClock,
    pub(super) fresh: bool,
}

fn judgement_of(v: Option<Judgement>) -> LeqJudgement {
    match v {
        None | Some(Judgement::Ok) => LeqJudgement::Ok,
        Some(Judgement::Near) => LeqJudgement::Near,
        Some(Judgement::Over) => LeqJudgement::Over,
    }
}

fn alarm_kind(prev: LeqJudgement, j: LeqJudgement) -> Option<LeqAlarmKind> {
    match (prev, j) {
        (p, LeqJudgement::Over) if p != LeqJudgement::Over => Some(LeqAlarmKind::Over),
        (LeqJudgement::Over, LeqJudgement::Ok | LeqJudgement::Near) => {
            Some(LeqAlarmKind::Recovered)
        }
        _ => None,
    }
}

impl BandMeter {
    /// A band meter for `cfg` at `fs` with the meter's Leq `horizon` (s). `None` (and the
    /// meter runs without bands) when the configuration does not run.
    pub(super) fn new(
        cfg: BandLeqConfig,
        fs: f64,
        horizon: u32,
        log: &SharedLog,
        local: LocalClock,
    ) -> Option<Self> {
        let seconds = cfg.seconds()?;
        let integ = match BandIntegrator::new(fs) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("band meter cannot run at {fs} Hz: {e}");
                return None;
            }
        };
        let windows = BandWindows::new(seconds, LF_BANDS, horizon);
        let empty = windows.windows().value(0);
        let state = BandState {
            value: empty,
            level_db: f64::NAN,
            limit_db: None,
            verdict: None,
            headroom: None,
        };
        let epoch = leq_log::lock(log).epoch();
        let mut m = Self {
            seconds,
            horizon,
            integ,
            windows,
            predicted: None,
            limits: conv::band_limits(&cfg),
            cfg,
            done: Vec::with_capacity(4),
            second_start: None,
            states: [state; LF_BANDS],
            latches: [Latch::default(); LF_BANDS],
            judgements: [LeqJudgement::NoLimit; LF_BANDS],
            predicted_latch: Latch::default(),
            predicted_judgement: LeqJudgement::NoLimit,
            at_horizon: Period::Day,
            epoch,
            local,
            fresh: true,
        };
        m.set_transfer();
        Some(m)
    }

    fn set_transfer(&mut self) {
        let dwelling = conv::band_limits(&self.cfg);
        match &self.cfg.transfer {
            Some(t) => {
                let t = conv::band_transfer(t);
                self.limits = t.foh_limits(&dwelling);
                let ch = [
                    (self.seconds, PredictedSecond::ESTIMATE),
                    (self.seconds, PredictedSecond::AT_MOST),
                ];
                self.predicted = Some((t, RollingLeq::of_channels(ch, self.horizon)));
            }
            None => {
                self.limits = dwelling;
                self.predicted = None;
            }
        }
    }

    /// Whether `cfg` can be taken without rebuilding the windows: only the §13 correction
    /// (which applies to the seconds from now on) or the warn margin differ.
    pub(super) fn same_windows(&self, cfg: &BandLeqConfig) -> bool {
        cfg.duration == self.cfg.duration
            && cfg.day == self.cfg.day
            && cfg.night == self.cfg.night
            && cfg.predicted == self.cfg.predicted
            && cfg.transfer == self.cfg.transfer
    }

    /// Headroom horizon, s.
    pub(super) fn horizon(&self) -> u32 {
        self.horizon
    }

    /// Takes a configuration [`Self::same_windows`] allows.
    pub(super) fn set_light(&mut self, cfg: BandLeqConfig) {
        self.cfg = cfg;
        self.fresh = true;
    }

    fn period_at(&self, wall_ns: u64) -> Period {
        Period::at(self.local.seconds_of_day(wall_ns))
    }

    /// Refills the windows (and the prediction) from the log's band rows of the window
    /// before `now` (wall ns), placed by wall time; gaps keep the period of their wall time.
    pub(super) fn rebuild(&mut self, log: &SharedLog, now: u64) {
        let placed = leq_log::lock(log).place_bands(now, self.seconds);
        let local = self.local;
        let period_at = move |ns: u64| Period::at(local.seconds_of_day(ns));
        self.windows.refill(placed.seconds(period_at));
        if let Some((t, ring)) = &mut self.predicted {
            ring.clear();
            for (s, _) in placed.seconds(period_at) {
                ring.push(t.predict(&s));
            }
        }
        self.fresh = true;
    }

    /// One completed second from wall `start` to `end` (ns): logged (when anything was
    /// measured), pushed into the windows with the correction in force, judged. Alarms
    /// raised go to `alarms`.
    pub(super) fn second(
        &mut self,
        s: &BandSecond,
        (start, end): (u64, u64),
        log: &SharedLog,
        sensitivity: Option<f64>,
        alarms: &mut Vec<LeqAlarm>,
    ) {
        let period = self.period_at(start);
        let correction = self.cfg.correction.db();
        let epoch = {
            let mut log = leq_log::lock(log);
            if s.measured > 0.0 {
                let mut levels = [0f32; ac2_core::band_leq::BANDS];
                for (i, l) in levels.iter_mut().enumerate() {
                    *l = s.level_dbfs(i) as f32;
                }
                log.push_band(BandLogRow {
                    start: WallNs(start),
                    measured: Seconds(s.measured),
                    levels,
                    correction: Db(correction),
                    period: conv::band_period(period),
                    sensitivity: sensitivity.map(Db),
                });
            }
            log.epoch()
        };
        if epoch != self.epoch {
            // `spl.log_new`: the windows start over with the new log.
            self.epoch = epoch;
            self.windows.clear();
            if let Some((_, ring)) = &mut self.predicted {
                ring.clear();
            }
            self.latches = [Latch::default(); LF_BANDS];
            self.judgements = [LeqJudgement::NoLimit; LF_BANDS];
            self.predicted_latch = Latch::default();
            self.predicted_judgement = LeqJudgement::NoLimit;
        }
        let c = s.corrected(correction);
        self.windows.push(c, period);
        if let Some((t, ring)) = &mut self.predicted {
            ring.push(t.predict(&c));
        }
        self.judge(WallNs(end), sensitivity, alarms);
    }

    /// Judges every band window and the predicted window at `at`: the limits of the period
    /// the windows are in, the headroom against those in force once the horizon has passed.
    /// Without a sensitivity a band with a limit is `not_calibrated` (the limits are SPL).
    pub(super) fn judge(
        &mut self,
        at: WallNs,
        sensitivity: Option<f64>,
        alarms: &mut Vec<LeqAlarm>,
    ) {
        let margin = self.cfg.warn_margin.0;
        let o = sensitivity.unwrap_or(0.0);
        self.at_horizon = self.period_at(at.0 + u64::from(self.horizon) * NS);
        self.windows
            .judge(&self.limits, o, margin, self.at_horizon, &mut self.states);
        let bands = self
            .states
            .iter_mut()
            .zip(&mut self.latches)
            .zip(&mut self.judgements)
            .zip(BAND_NOMINAL_HZ);
        for (((st, latch), judgement), nominal) in bands {
            let (j, verdict) = match (st.limit_db, sensitivity) {
                (None, _) => {
                    *latch = Latch::default();
                    (LeqJudgement::NoLimit, None)
                }
                (Some(_), None) => {
                    *latch = Latch::default();
                    (LeqJudgement::NotCalibrated, None)
                }
                (Some(limit), Some(_)) => {
                    let v = latch.judge(st.verdict, st.level_db, limit, margin);
                    (judgement_of(v.map(|v| v.judgement)), v)
                }
            };
            // The worst band and the frame go by the held state, as the alarms do.
            st.verdict = verdict;
            let prev = std::mem::replace(judgement, j);
            if j == prev {
                continue;
            }
            if let (Some(kind), Some(limit)) = (alarm_kind(prev, j), st.limit_db) {
                alarms.push(LeqAlarm {
                    at,
                    subject: AlarmSubject::Band {
                        nominal: Hz(nominal),
                    },
                    kind,
                    level: DbSpl(st.level_db),
                    limit: DbSpl(limit),
                    position: None,
                });
            }
        }
        let limit = self.predicted_limit();
        let j = match (&self.predicted, limit, sensitivity) {
            (None, ..) | (_, None, _) => {
                self.predicted_latch = Latch::default();
                LeqJudgement::NoLimit
            }
            (Some(_), Some(_), None) => {
                self.predicted_latch = Latch::default();
                LeqJudgement::NotCalibrated
            }
            (Some((_, ring)), Some(l), Some(o)) => {
                let v = ring.value(PredictedSecond::ESTIMATE);
                let raw = judge_window(&v, o, l.0, margin);
                let held = self.predicted_latch.judge(raw, v.leq_dbfs + o, l.0, margin);
                judgement_of(held.map(|v| v.judgement))
            }
        };
        let prev = std::mem::replace(&mut self.predicted_judgement, j);
        if let (Some(kind), Some(l), Some((_, ring))) =
            (alarm_kind(prev, j), limit, &self.predicted)
            && j != prev
        {
            alarms.push(LeqAlarm {
                at,
                subject: AlarmSubject::Predicted,
                kind,
                level: DbSpl(ring.value(PredictedSecond::ESTIMATE).leq_dbfs + o),
                limit: l,
                position: None,
            });
        }
        self.fresh = true;
    }

    /// The predicted LAeq's limit in force for the windows' period.
    fn predicted_limit(&self) -> Option<DbSpl> {
        match self.windows.period() {
            Period::Day => self.cfg.predicted.day,
            Period::Night => self.cfg.predicted.night,
        }
    }

    /// The `band_leq` frame's metadata: levels in dB SPL with a sensitivity, else dBFS.
    pub(super) fn meta(
        &self,
        sensitivity: Option<f64>,
        cal: CalStatus,
        mic_curve: bool,
    ) -> BandLeqMeta {
        let o = sensitivity.unwrap_or(0.0);
        let judged = sensitivity.is_some();
        let bands = (0..LF_BANDS)
            .map(|i| {
                let st = &self.states[i];
                let (mut allowed, mut recover) = (None, None);
                if judged {
                    match st.headroom {
                        Some(Headroom::Allowed { ms }) => allowed = Some(power_dbfs(ms) + o),
                        Some(Headroom::CannotRecover { recover_s }) => {
                            recover = Some(Seconds(f64::from(recover_s)));
                        }
                        None => {}
                    }
                }
                BandLeqBand {
                    nominal: Hz(BAND_NOMINAL_HZ[i]),
                    leq: st.level_db,
                    limit: st.limit_db,
                    judgement: self.judgements[i],
                    on_course: st.verdict.is_some_and(|v| v.on_course),
                    allowed,
                    recover,
                }
            })
            .collect();
        let v = self.windows.windows().value(0);
        let predicted = self.predicted.as_ref().map(|(_, ring)| {
            let at = |ch: usize| {
                if judged {
                    ring.value(ch).leq_dbfs + o
                } else {
                    f64::NAN
                }
            };
            PredictedLeq {
                estimate: at(PredictedSecond::ESTIMATE),
                at_most: at(PredictedSecond::AT_MOST),
                limit: self.predicted_limit(),
                judgement: self.predicted_judgement,
            }
        });
        BandLeqMeta {
            scale: if judged {
                LevelScale::DbSpl
            } else {
                LevelScale::Dbfs
            },
            cal,
            mic_curve,
            duration: self.cfg.duration,
            horizon: Seconds(f64::from(self.horizon)),
            elapsed: Seconds(f64::from(v.elapsed)),
            measured: Seconds(v.measured),
            period: conv::band_period(self.windows.period()),
            period_after_horizon: conv::band_period(
                self.windows.period_after_horizon(self.at_horizon),
            ),
            correction: Db(self.cfg.correction.db()),
            limits_from: if self.predicted.is_some() {
                BandLimitPlace::Transferred
            } else {
                BandLimitPlace::AtMic
            },
            bands,
            worst: worst_band(&self.states).map(|i| i as u8),
            predicted,
        }
    }
}
