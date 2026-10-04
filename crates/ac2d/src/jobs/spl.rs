//! Sound level meter job (`spl` and `leq` frames). Levels are dBFS, or dB SPL when a
//! sensitivity calibration applies to the input; the input's mic curve, when on, runs as a
//! minimum-phase filter before frequency weighting (`docs/design/q7-calibration.md` §6).
//!
//! Besides the meter, every second goes into the meter's log and its rolling Leq windows
//! (`docs/design/leq.md`); a window going over its limit or recovering is reported to the
//! control thread, which keeps the `spl_log` entity. Freeze and reset are display
//! operations of the meter: the log and the windows carry on through them.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use ac2_core::leq::{Headroom, RollingLeq, Second, SecondIntegrator, WindowSpec, judge};
use ac2_core::mic_curve::Correction;
use ac2_core::spectrum::power_dbfs;
use ac2_core::spl::{Sensitivity, SplMeter, SplMeterConfig};
use ac2_proto::frame::{
    FrameData, LeqFlags, LeqFrame, LeqMeta, LeqRun, ProtectionFlags, SplFrame, SplMeta,
};
use ac2_proto::model::{
    LeqAlarm, LeqAlarmKind, LeqConfig, LeqJudgement, LevelScale, SplConfig, SplLogRow,
};
use ac2_proto::units::{DbSpl, Dbfs, MeasId, Rev, Seconds, WallNs};

use super::{Analysis, Emitter, JobCmd, LevelsMeter, StampArgs, channel_f64};
use crate::calstore::InputCal;
use crate::control::ControlMsg;
use crate::conv;
use crate::fanout::Block;
use crate::leq_log::{self, SharedLog};

const NS: f64 = 1e9;

pub(crate) struct Spl {
    meas: MeasId,
    cfg: SplConfig,
    fs: f64,
    idx: usize,
    meter: SplMeter,
    cal: InputCal,
    frozen: bool,
    config_rev: Rev,
    applied_at: Option<u64>,
    levels: LevelsMeter,
    buf: Vec<f64>,
    end: Option<u64>,
    wall: u64,
    leq: LeqWindows,
}

/// The meter's one-second integration, its log and its rolling windows.
struct LeqWindows {
    seconds: SecondIntegrator,
    ring: RollingLeq,
    cfg: LeqConfig,
    log: SharedLog,
    to_control: Sender<ControlMsg>,
    /// Sample index of the current second's first sample; `None` before the first block.
    second_start: Option<u64>,
    /// One past the newest sample integrated.
    next: u64,
    /// Wall clock of `next` (ns), from the newest block.
    next_wall: u64,
    judgements: Vec<LeqJudgement>,
    /// The log's epoch the windows and judgements belong to.
    epoch: u64,
    done: Vec<Second>,
    /// A second completed since the last `leq` frame.
    fresh: bool,
}

fn same_curve(a: Option<&Arc<Correction>>, b: Option<&Arc<Correction>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
        _ => false,
    }
}

fn specs(cfg: &LeqConfig) -> Vec<WindowSpec> {
    cfg.windows
        .iter()
        .map(|w| WindowSpec {
            seconds: w.seconds().unwrap_or(1),
            weighting: conv::weighting(w.weighting),
        })
        .collect()
}

fn ring_for(cfg: &LeqConfig) -> RollingLeq {
    RollingLeq::new(&specs(cfg), cfg.horizon_seconds().unwrap_or(60))
}

/// What the job needs to keep a meter's log.
pub(crate) struct LeqSetup {
    pub(crate) log: SharedLog,
    pub(crate) to_control: Sender<ControlMsg>,
    /// The windows' judgements as the `spl_log` entity holds them: a restarted job
    /// reports only what changes from there.
    pub(crate) judgements: Vec<LeqJudgement>,
}

impl LeqWindows {
    fn new(cfg: LeqConfig, fs: f64, setup: LeqSetup) -> Result<Self, String> {
        let n = cfg.windows.len();
        let mut judgements = setup.judgements;
        judgements.resize(n, LeqJudgement::NoLimit);
        let epoch = leq_log::lock(&setup.log).epoch();
        Ok(Self {
            seconds: SecondIntegrator::new(fs).map_err(|e| e.to_string())?,
            ring: ring_for(&cfg),
            cfg,
            log: setup.log,
            to_control: setup.to_control,
            second_start: None,
            next: 0,
            next_wall: 0,
            judgements,
            epoch,
            done: Vec::with_capacity(4),
            fresh: false,
        })
    }

    /// Wall clock of sample `s` (ns), from the newest block's.
    fn wall_of(&self, s: u64, fs: f64) -> u64 {
        let ds = (s as f64 - self.next as f64) / fs * NS;
        (self.next_wall as f64 + ds).max(0.0) as u64
    }

    fn set_config(&mut self, cfg: LeqConfig, fs: f64) {
        let old_cfg = std::mem::replace(&mut self.cfg, cfg);
        let old = std::mem::take(&mut self.judgements);
        // A window configured as before keeps its judgement, so an unchanged window
        // reports no transition.
        self.judgements = self
            .cfg
            .windows
            .iter()
            .map(|w| {
                old_cfg
                    .windows
                    .iter()
                    .position(|o| o == w)
                    .and_then(|i| old.get(i).copied())
                    .unwrap_or(LeqJudgement::NoLimit)
            })
            .collect();
        self.ring = ring_for(&self.cfg);
        if let Some(start) = self.second_start {
            let now = self.wall_of(start, fs);
            leq_log::lock(&self.log).rebuild(&mut self.ring, now);
        }
        self.fresh = true;
    }
}

impl Spl {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        meas: MeasId,
        cfg: SplConfig,
        sample_rate: u32,
        idx: usize,
        cal: InputCal,
        frozen: bool,
        config_rev: Rev,
        leq: LeqSetup,
    ) -> Result<Self, String> {
        let fs = f64::from(sample_rate);
        let mut meter = SplMeter::new(SplMeterConfig {
            fs,
            weighting: conv::weighting(cfg.weighting),
            time_weighting: conv::time_weighting(cfg.time_weighting),
            peak_weighting: conv::peak_weighting(cfg.peak_weighting),
        })
        .map_err(|e| e.to_string())?;
        let mut leq = LeqWindows::new(cfg.leq.clone(), fs, leq)?;
        if let Some(c) = &cal.correction {
            let taps = c.design_fir(fs);
            meter.set_correction(Some(&taps));
            leq.seconds.set_correction(Some(&taps));
        }
        Ok(Self {
            levels: LevelsMeter::new(vec![idx], vec![cfg.input], sample_rate),
            meas,
            cfg,
            fs,
            idx,
            meter,
            cal,
            frozen,
            config_rev,
            applied_at: None,
            buf: Vec::new(),
            end: None,
            wall: 0,
            leq,
        })
    }

    fn set_cal(&mut self, cal: InputCal) {
        if !same_curve(self.cal.correction.as_ref(), cal.correction.as_ref()) {
            let taps = cal.correction.as_ref().map(|c| c.design_fir(self.fs));
            self.meter.set_correction(taps.as_deref());
            self.leq.seconds.set_correction(taps.as_deref());
        }
        self.cal = cal;
    }

    /// Feeds the block to the one-second integration; logs and judges every completed
    /// second.
    fn push_leq(&mut self, b: &Block) {
        let fs = self.fs;
        let l = &mut self.leq;
        let block_wall = (b.wall_ns as f64 - f64::from(b.frames) / fs * NS).max(0.0) as u64;
        match l.second_start {
            None => {
                // The windows carry on from the log: what it holds from before this job
                // (another run of the meter, a loaded session) is placed by wall time.
                l.second_start = Some(b.start_sample);
                l.next = b.start_sample;
                l.next_wall = block_wall;
                leq_log::lock(&l.log).rebuild(&mut l.ring, block_wall);
            }
            Some(_) if b.start_sample > l.next => {
                // Lost samples: the second grid moves on without energy or measured time.
                let lost = b.start_sample - l.next;
                let done = &mut l.done;
                l.seconds.skip(lost, |s| done.push(s));
            }
            Some(_) => {}
        }
        l.next = b.start_sample;
        l.next_wall = block_wall;
        let done = &mut l.done;
        l.seconds.process(&self.buf, |s| done.push(s));
        l.next = b.end_sample();
        l.next_wall = b.wall_ns;
        if l.done.is_empty() {
            return;
        }
        let per = self.leq.seconds.samples_per_second();
        let seconds = std::mem::take(&mut self.leq.done);
        for s in &seconds {
            let start = self.leq.second_start.unwrap_or(0);
            let end = start + per;
            self.leq.second_start = Some(end);
            let row = (s.measured > 0.0).then(|| SplLogRow {
                start: WallNs(self.leq.wall_of(start, fs)),
                measured: Seconds(s.measured),
                laeq: Dbfs(s.level_dbfs(ac2_core::weighting::Weighting::A)),
                lceq: Dbfs(s.level_dbfs(ac2_core::weighting::Weighting::C)),
                lzeq: Dbfs(s.level_dbfs(ac2_core::weighting::Weighting::Z)),
                sensitivity: self.cal.sensitivity.map(ac2_proto::units::Db),
            });
            let (epoch, first) = {
                let mut log = leq_log::lock(&self.leq.log);
                if let Some(row) = row {
                    log.push(row);
                }
                (log.epoch(), row.is_some() && log.total() == 1)
            };
            if epoch != self.leq.epoch {
                // `spl.log_new` started a new log: the windows and their states start over
                // with it (the control thread reset the entity to the same states).
                self.leq.epoch = epoch;
                self.leq.ring.clear();
                let calibrated = self.cal.sensitivity.is_some();
                self.leq.judgements = self
                    .leq
                    .cfg
                    .windows
                    .iter()
                    .map(|w| leq_log::initial_judgement(w, calibrated))
                    .collect();
            }
            self.leq.ring.push(*s);
            if first {
                self.report(WallNs(self.leq.wall_of(end, fs)), Vec::new());
            }
            self.judge(WallNs(self.leq.wall_of(end, fs)));
        }
        self.leq.done = seconds;
        self.leq.done.clear();
        self.leq.fresh = true;
    }

    /// Judges every window after a second; transitions go to the control thread.
    fn judge(&mut self, at: WallNs) {
        let offset = self.cal.sensitivity;
        let mut alarms = Vec::new();
        let mut changed = false;
        for (i, w) in self.leq.cfg.windows.iter().enumerate() {
            let v = self.leq.ring.value(i);
            let j = match (w.limit, offset) {
                (None, _) => LeqJudgement::NoLimit,
                (Some(_), None) => LeqJudgement::NotCalibrated,
                (Some(limit), Some(o)) => match judge(v.leq_dbfs + o, limit.0, w.warn_margin.0) {
                    None => LeqJudgement::Ok,
                    Some(ac2_core::leq::Judgement::Ok) => LeqJudgement::Ok,
                    Some(ac2_core::leq::Judgement::Near) => LeqJudgement::Near,
                    Some(ac2_core::leq::Judgement::Over) => LeqJudgement::Over,
                },
            };
            let prev = self.leq.judgements[i];
            if j == prev {
                continue;
            }
            changed = true;
            self.leq.judgements[i] = j;
            let kind = match (prev, j) {
                (p, LeqJudgement::Over) if p != LeqJudgement::Over => Some(LeqAlarmKind::Over),
                (LeqJudgement::Over, LeqJudgement::Ok | LeqJudgement::Near) => {
                    Some(LeqAlarmKind::Recovered)
                }
                _ => None,
            };
            if let (Some(kind), Some(limit), Some(o)) = (kind, w.limit, offset) {
                alarms.push(LeqAlarm {
                    at,
                    duration: w.duration,
                    weighting: w.weighting,
                    kind,
                    leq: DbSpl(v.leq_dbfs + o),
                    limit,
                });
            }
        }
        if changed {
            self.report(at, alarms);
        }
    }

    fn report(&self, at: WallNs, alarms: Vec<LeqAlarm>) {
        let _ = self.leq.to_control.send(ControlMsg::Leq {
            meas: self.meas,
            epoch: self.leq.epoch,
            config_rev: self.config_rev,
            at,
            judgements: self.leq.judgements.clone(),
            alarms,
        });
    }

    fn leq_frame(&self) -> LeqFrame {
        let offset = self.cal.sensitivity;
        let o = offset.unwrap_or(0.0);
        let n = self.leq.cfg.windows.len();
        let (logged, run) = {
            let log = leq_log::lock(&self.leq.log);
            (log.total(), log.run())
        };
        let mut f = LeqFrame {
            meas: self.meas,
            meta: LeqMeta {
                scale: if offset.is_some() {
                    LevelScale::DbSpl
                } else {
                    LevelScale::Dbfs
                },
                cal: self.cal.status,
                mic_curve: self.leq.seconds.has_correction(),
                horizon: self.leq.cfg.horizon,
                logged,
                run: run.map(|r| LeqRun {
                    started_at: r.started_at,
                    until: r.until,
                    measured: Seconds(r.measured),
                    gaps: Seconds(r.gaps),
                    trimmed: r.trimmed,
                    // In the meter's scale now, as the windows: the energies are dBFS.
                    laeq: r.levels_dbfs[0] + o,
                    lceq: r.levels_dbfs[1] + o,
                    lzeq: r.levels_dbfs[2] + o,
                }),
            },
            leq: Vec::with_capacity(n),
            elapsed: Vec::with_capacity(n),
            measured: Vec::with_capacity(n),
            allowed: Vec::with_capacity(n),
            recover: Vec::with_capacity(n),
            flags: Vec::with_capacity(n),
        };
        for (i, w) in self.leq.cfg.windows.iter().enumerate() {
            let v = self.leq.ring.value(i);
            let mut flags = LeqFlags::NONE;
            let (mut allowed, mut recover) = (f32::NAN, f32::NAN);
            if let Some(limit) = w.limit {
                flags = flags.with(LeqFlags::LIMIT);
                if offset.is_some() {
                    flags = flags.with(LeqFlags::JUDGED);
                    match self.leq.judgements.get(i) {
                        Some(LeqJudgement::Over) => flags = flags.with(LeqFlags::OVER),
                        Some(LeqJudgement::Near) => flags = flags.with(LeqFlags::NEAR),
                        _ => {}
                    }
                    let p = ac2_core::leq::mean_square(limit.0 - o);
                    match self.leq.ring.headroom(i, p) {
                        Headroom::Allowed { ms } => allowed = (power_dbfs(ms) + o) as f32,
                        Headroom::CannotRecover { recover_s } => {
                            flags = flags.with(LeqFlags::CANNOT_RECOVER);
                            recover = recover_s as f32;
                        }
                    }
                }
            }
            if v.incomplete() {
                flags = flags.with(LeqFlags::INCOMPLETE);
            }
            f.leq.push((v.leq_dbfs + o) as f32);
            f.elapsed.push(v.elapsed as f32);
            f.measured.push(v.measured as f32);
            f.allowed.push(allowed);
            f.recover.push(recover);
            f.flags.push(flags);
        }
        f
    }
}

impl Analysis for Spl {
    fn push(&mut self, b: &Block) {
        self.applied_at.get_or_insert(b.start_sample);
        channel_f64(b, self.idx, &mut self.buf);
        if !self.frozen {
            self.meter.process(&self.buf);
        }
        self.push_leq(b);
        self.levels.push(b);
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
    }

    fn command(&mut self, c: JobCmd) {
        match c {
            JobCmd::Freeze(f) => self.frozen = f,
            JobCmd::Reset => self.meter.reset_interval(),
            JobCmd::Cal(cal) => self.set_cal(*cal),
            JobCmd::Spl { config, rev } => {
                self.config_rev = rev;
                self.meter.select(
                    conv::weighting(config.weighting),
                    conv::time_weighting(config.time_weighting),
                    conv::peak_weighting(config.peak_weighting),
                );
                let leq_changed = config.leq != self.cfg.leq;
                self.cfg = *config;
                if !leq_changed {
                    return;
                }
                self.leq.set_config(self.cfg.leq.clone(), self.fs);
                // The rebuilt windows are judged at once: a new limit below the level is
                // an alarm now, not a second later, and the next frame carries the state.
                let at = if self.wall > 0 {
                    self.wall
                } else {
                    crate::util::wall_ns()
                };
                self.judge(WallNs(at));
            }
            JobCmd::SetDelay { .. }
            | JobCmd::Find { .. }
            | JobCmd::Track { .. }
            | JobCmd::Smoothing { .. } => {}
        }
    }

    fn emit(&mut self, e: &Emitter) {
        let Some(end) = self.end else {
            return;
        };
        let mut l = self.meter.levels();
        let scale = match self.cal.sensitivity {
            Some(offset_db) => {
                l = l.calibrated(Sensitivity { offset_db });
                LevelScale::DbSpl
            }
            None => LevelScale::Dbfs,
        };
        let stamp = StampArgs {
            audio_sample: end.saturating_sub(1),
            config_rev: self.config_rev,
            applied_at: self.applied_at.unwrap_or(0),
            wall_ns: self.wall,
            grid_id: None,
            protection: if self.levels.any_clip_held() {
                ProtectionFlags::CLIP
            } else {
                ProtectionFlags::NONE
            },
        };
        e.send(
            stamp,
            FrameData::Spl(SplFrame {
                meas: self.meas,
                meta: SplMeta {
                    scale,
                    weighting: self.cfg.weighting,
                    time_weighting: self.cfg.time_weighting,
                    peak_weighting: self.cfg.peak_weighting,
                    level: l.level,
                    lmax: l.lmax,
                    lmin: l.lmin,
                    leq: l.leq,
                    lpeak: l.lpeak,
                    duration: Seconds(l.duration_s),
                    cal: self.cal.status,
                    mic_curve: self.meter.has_correction(),
                },
            }),
        );
        if self.leq.fresh {
            self.leq.fresh = false;
            e.send(stamp, FrameData::Leq(self.leq_frame()));
        }
        if let Some(lv) = self.levels.take(self.meas) {
            e.send(stamp, FrameData::Levels(lv));
        }
    }
}
