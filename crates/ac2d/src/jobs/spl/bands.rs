//! The band meter of an SPL meter (`docs/design/band-leq.md`): the unweighted, mic-curve
//! corrected signal through the 1/3-octave bank, one [`BandSecond`] per second of the
//! meter's own second grid, logged next to its SPL log (every band, unweighted), the band
//! windows on the shown bands, each of its own length and weighting, judged against the
//! limits of the period in force, and the level at the transfer's place predicted through
//! the transfer.

use ac2_core::band_leq::{
    BANDS, BandIntegrator, BandLimits, BandSecond, BandState, BandWindows, Period, PredictedSecond,
    Transfer, worst_band,
};
use ac2_core::leq::{Headroom, Judgement, Latch, RollingLeq, judge_window};
use ac2_core::spectrum::power_dbfs;
use ac2_proto::frame::{BandLeqFrame, BandLeqMeta, BandWindowState, LeqFlags};
use ac2_proto::model::{
    AlarmSubject, BAND_NOMINAL_HZ, BandLeqConfig, BandLimitPlace, BandLimitSet, BandWindow,
    CalStatus, LeqAlarm, LeqAlarmKind, LeqJudgement, LevelScale, PredictedLeq, PredictedWindow,
    TransferOrigin,
};
use ac2_proto::units::{Db, DbSpl, Hz, MeasId, Seconds, WallNs};
use ac2_traces::band_log::BandLogRow;

use crate::config::LocalClock;
use crate::conv;
use crate::leq_log::{self, SharedLog};

const NS: u64 = 1_000_000_000;

/// One band window between seconds.
struct Win {
    spec: BandWindow,
    seconds: u32,
    windows: BandWindows,
    /// The limits judged at the mic: the window's, plus each band's attenuation with a
    /// transfer.
    limits: BandLimits,
    /// Per shown band.
    states: Vec<BandState>,
    latches: Vec<Latch>,
    judgements: Vec<LeqJudgement>,
    at_horizon: Period,
}

/// The predicted window at the transfer's place.
struct Predicted {
    spec: PredictedWindow,
    seconds: u32,
    transfer: Transfer,
    ring: RollingLeq<PredictedSecond>,
    /// Seconds pushed when the newest night second was pushed.
    last_night: Option<u64>,
    latch: Latch,
    judgement: LeqJudgement,
}

impl Predicted {
    fn push(&mut self, s: &BandSecond, period: Period) {
        self.ring.push(self.transfer.predict(s));
        if period == Period::Night {
            self.last_night = Some(self.ring.pushed());
        }
    }

    fn clear(&mut self) {
        self.ring.clear();
        self.last_night = None;
    }

    /// Night while the window holds a night second (as a band window).
    fn period(&self) -> Period {
        match self.last_night {
            Some(n) if self.ring.pushed() - n < u64::from(self.seconds) => Period::Night,
            _ => Period::Day,
        }
    }

    fn limit(&self) -> Option<DbSpl> {
        match self.period() {
            Period::Day => self.spec.day,
            Period::Night => self.spec.night,
        }
    }
}

/// A band meter's state between seconds.
pub(super) struct BandMeter {
    pub(super) cfg: BandLeqConfig,
    horizon: u32,
    pub(super) integ: BandIntegrator,
    /// The shown bands, indices into [`BAND_NOMINAL_HZ`].
    bands: Vec<usize>,
    wins: Vec<Win>,
    predicted: Option<Predicted>,
    /// Completed seconds of the block being processed (capacity kept between blocks).
    pub(super) done: Vec<BandSecond>,
    /// Sample at which the current second started; `None` until the first block.
    pub(super) second_start: Option<u64>,
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

/// The core's limits of a window, dB SPL, on its 28 bands.
fn limits_of(set: &BandLimitSet) -> BandLimits {
    let night = set.night().map(|l| l.map(|l| l.0));
    match set.day_offset() {
        Some(o) => BandLimits::night_and_offset_day(night, o.0),
        None => BandLimits::always(night),
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
        if let Err(e) = cfg.check() {
            tracing::warn!("band meter not run: {e}");
            return None;
        }
        let bands = cfg.band_indices()?;
        let integ = match BandIntegrator::new(fs) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("band meter cannot run at {fs} Hz: {e}");
                return None;
            }
        };
        // A band without a transfer has no limit at the mic: its attenuation is unknown.
        let transfer = cfg.transfer.as_ref().map(conv::band_transfer);
        let wins = cfg
            .windows
            .iter()
            .filter_map(|spec| {
                let seconds = spec.seconds()?;
                let windows =
                    BandWindows::new(seconds, &bands, conv::weighting(spec.weighting), horizon);
                let own = limits_of(&spec.limits);
                let empty = BandState {
                    value: windows.windows().value(0),
                    level_db: f64::NAN,
                    limit_db: None,
                    verdict: None,
                    headroom: None,
                };
                Some(Win {
                    spec: *spec,
                    seconds,
                    limits: transfer.as_ref().map_or(own, |t| t.foh_limits(&own)),
                    windows,
                    states: vec![empty; bands.len()],
                    latches: vec![Latch::default(); bands.len()],
                    judgements: vec![LeqJudgement::NoLimit; bands.len()],
                    at_horizon: Period::Day,
                })
            })
            .collect();
        let predicted = transfer.zip(cfg.predicted).and_then(|(t, spec)| {
            let seconds = cfg.predicted_seconds()?;
            let ch = [
                (seconds, PredictedSecond::ESTIMATE),
                (seconds, PredictedSecond::AT_MOST),
            ];
            Some(Predicted {
                spec,
                seconds,
                transfer: t,
                ring: RollingLeq::of_channels(ch, horizon),
                last_night: None,
                latch: Latch::default(),
                judgement: LeqJudgement::NoLimit,
            })
        });
        let epoch = leq_log::lock(log).epoch();
        Some(Self {
            horizon,
            integ,
            bands,
            wins,
            predicted,
            cfg,
            done: Vec::with_capacity(4),
            second_start: None,
            epoch,
            local,
            fresh: true,
        })
    }

    /// Whether `cfg` can be taken without rebuilding the windows: only the §13 correction
    /// (which applies to the seconds from now on) or the warn margins differ.
    pub(super) fn same_windows(&self, cfg: &BandLeqConfig) -> bool {
        let shape = |c: &BandLeqConfig| {
            (
                c.windows
                    .iter()
                    .map(|w| (w.duration, w.weighting, w.limits))
                    .collect::<Vec<_>>(),
                c.bands.clone(),
                c.predicted.map(|p| (p.duration, p.day, p.night)),
                c.transfer.clone(),
            )
        };
        shape(cfg) == shape(&self.cfg)
    }

    /// Headroom horizon, s.
    pub(super) fn horizon(&self) -> u32 {
        self.horizon
    }

    /// Takes a configuration [`Self::same_windows`] allows.
    pub(super) fn set_light(&mut self, cfg: BandLeqConfig) {
        for (w, spec) in self.wins.iter_mut().zip(&cfg.windows) {
            w.spec = *spec;
        }
        if let (Some(p), Some(spec)) = (&mut self.predicted, cfg.predicted) {
            p.spec = spec;
        }
        self.cfg = cfg;
        self.fresh = true;
    }

    fn period_at(&self, wall_ns: u64) -> Period {
        Period::at(self.local.seconds_of_day(wall_ns))
    }

    /// Refills every window (and the prediction) from the log's band rows of its length
    /// before `now` (wall ns), placed by wall time; gaps keep the period of their wall time.
    /// The log holds every band unweighted, so any window, weighting or band selection is
    /// rebuilt from it.
    pub(super) fn rebuild(&mut self, log: &SharedLog, now: u64) {
        let local = self.local;
        let period_at = move |ns: u64| Period::at(local.seconds_of_day(ns));
        let log = leq_log::lock(log);
        for w in &mut self.wins {
            let placed = log.place_bands(now, w.seconds);
            w.windows.refill(placed.seconds(period_at));
        }
        if let Some(p) = &mut self.predicted {
            let placed = log.place_bands(now, p.seconds);
            p.clear();
            for (s, period) in placed.seconds(period_at) {
                p.push(&s, period);
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
                let mut levels = [0f32; BANDS];
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
            for w in &mut self.wins {
                w.windows.clear();
                w.latches.fill(Latch::default());
                w.judgements.fill(LeqJudgement::NoLimit);
            }
            if let Some(p) = &mut self.predicted {
                p.clear();
                p.latch = Latch::default();
                p.judgement = LeqJudgement::NoLimit;
            }
        }
        let c = s.corrected(correction);
        for w in &mut self.wins {
            w.windows.push(c, period);
        }
        if let Some(p) = &mut self.predicted {
            p.push(&c, period);
        }
        self.judge(WallNs(end), sensitivity, alarms);
    }

    /// Judges every band of every window and the predicted window at `at`: the limits of
    /// the period each window is in, the headroom against those in force once the horizon
    /// has passed. Without a sensitivity a band with a limit is `not_calibrated` (the
    /// limits are SPL).
    pub(super) fn judge(
        &mut self,
        at: WallNs,
        sensitivity: Option<f64>,
        alarms: &mut Vec<LeqAlarm>,
    ) {
        let o = sensitivity.unwrap_or(0.0);
        let at_horizon = self.period_at(at.0 + u64::from(self.horizon) * NS);
        for w in &mut self.wins {
            let margin = w.spec.warn_margin.0;
            w.at_horizon = at_horizon;
            w.windows
                .judge(&w.limits, o, margin, at_horizon, &mut w.states);
            let bands = w
                .states
                .iter_mut()
                .zip(&mut w.latches)
                .zip(&mut w.judgements)
                .zip(&self.bands);
            for (((st, latch), judgement), &band) in bands {
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
                            duration: w.spec.duration,
                            weighting: w.spec.weighting,
                            nominal: Hz(BAND_NOMINAL_HZ[band]),
                        },
                        kind,
                        level: DbSpl(st.level_db),
                        limit: DbSpl(limit),
                        position: None,
                    });
                }
            }
        }
        if let Some(p) = &mut self.predicted {
            let margin = p.spec.warn_margin.0;
            let limit = p.limit();
            let j = match (limit, sensitivity) {
                (None, _) => {
                    p.latch = Latch::default();
                    LeqJudgement::NoLimit
                }
                (Some(_), None) => {
                    p.latch = Latch::default();
                    LeqJudgement::NotCalibrated
                }
                (Some(l), Some(o)) => {
                    let v = p.ring.value(PredictedSecond::ESTIMATE);
                    let raw = judge_window(&v, o, l.0, margin);
                    let held = p.latch.judge(raw, v.leq_dbfs + o, l.0, margin);
                    judgement_of(held.map(|v| v.judgement))
                }
            };
            let prev = std::mem::replace(&mut p.judgement, j);
            if let (Some(kind), Some(l)) = (alarm_kind(prev, j), limit)
                && j != prev
            {
                alarms.push(LeqAlarm {
                    at,
                    subject: AlarmSubject::Predicted,
                    kind,
                    level: DbSpl(p.ring.value(PredictedSecond::ESTIMATE).leq_dbfs + o),
                    limit: l,
                    position: None,
                });
            }
        }
        self.fresh = true;
    }

    /// The `band_leq` frame: levels in dB SPL with a sensitivity, else dBFS.
    pub(super) fn frame(
        &self,
        meas: MeasId,
        sensitivity: Option<f64>,
        cal: CalStatus,
        mic_curve: bool,
    ) -> BandLeqFrame {
        let o = sensitivity.unwrap_or(0.0);
        let judged = sensitivity.is_some();
        let n = self.wins.len() * self.bands.len();
        let (mut leq, mut limit, mut allowed, mut recover, mut flags) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let mut windows = Vec::with_capacity(self.wins.len());
        for w in &self.wins {
            for (st, &j) in w.states.iter().zip(&w.judgements) {
                let (mut a, mut r) = (f32::NAN, f32::NAN);
                let mut f = LeqFlags::NONE;
                if judged {
                    match st.headroom {
                        Some(Headroom::Allowed { ms }) => a = (power_dbfs(ms) + o) as f32,
                        Some(Headroom::CannotRecover { recover_s }) => {
                            r = recover_s as f32;
                            f = f.with(LeqFlags::CANNOT_RECOVER);
                        }
                        None => {}
                    }
                }
                f = f.with(match j {
                    LeqJudgement::NoLimit => LeqFlags::NONE,
                    LeqJudgement::NotCalibrated => LeqFlags::LIMIT,
                    LeqJudgement::Ok => LeqFlags::LIMIT.with(LeqFlags::JUDGED),
                    LeqJudgement::Near => {
                        LeqFlags::LIMIT.with(LeqFlags::JUDGED).with(LeqFlags::NEAR)
                    }
                    LeqJudgement::Over => {
                        LeqFlags::LIMIT.with(LeqFlags::JUDGED).with(LeqFlags::OVER)
                    }
                });
                if st.verdict.is_some_and(|v| v.on_course) {
                    f = f.with(LeqFlags::ON_COURSE);
                }
                if st.value.incomplete() {
                    f = f.with(LeqFlags::INCOMPLETE);
                }
                leq.push(st.level_db as f32);
                limit.push(st.limit_db.map_or(f32::NAN, |l| l as f32));
                allowed.push(a);
                recover.push(r);
                flags.push(f);
            }
            let v = w.windows.windows().value(0);
            windows.push(BandWindowState {
                duration: w.spec.duration,
                weighting: w.spec.weighting,
                elapsed: Seconds(f64::from(v.elapsed)),
                measured: Seconds(v.measured),
                period: conv::band_period(w.windows.period()),
                period_after_horizon: conv::band_period(
                    w.windows.period_after_horizon(w.at_horizon),
                ),
                worst: worst_band(&w.states).map(|i| i as u8),
            });
        }
        let predicted = self.predicted.as_ref().map(|p| {
            let at = |ch: usize| {
                if judged {
                    p.ring.value(ch).leq_dbfs + o
                } else {
                    f64::NAN
                }
            };
            PredictedLeq {
                duration: p.spec.duration,
                estimate: at(PredictedSecond::ESTIMATE),
                at_most: at(PredictedSecond::AT_MOST),
                limit: p.limit(),
                judgement: p.judgement,
            }
        });
        let meta = BandLeqMeta {
            scale: if judged {
                LevelScale::DbSpl
            } else {
                LevelScale::Dbfs
            },
            cal,
            mic_curve,
            horizon: Seconds(f64::from(self.horizon)),
            correction: Db(self.cfg.correction.db()),
            limits_from: match self.cfg.transfer.as_ref().map(|t| t.origin) {
                None => BandLimitPlace::AtMic,
                Some(TransferOrigin::Estimated) => BandLimitPlace::Estimated,
                Some(TransferOrigin::Measured) => BandLimitPlace::Transferred,
            },
            bands: self.bands.iter().map(|&b| b as u8).collect(),
            windows,
            predicted,
        };
        BandLeqFrame {
            meas,
            meta,
            leq,
            limit,
            allowed,
            recover,
            flags,
        }
    }
}
