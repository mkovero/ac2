//! The SPL job's `spl` and `leq` frames, from the meter and the windows as they stand.

use ac2_core::leq::Headroom;
use ac2_core::spectrum::power_dbfs;
use ac2_core::spl::Sensitivity;
use ac2_proto::frame::{LeqFlags, LeqFrame, LeqMeta, LeqPeak, LeqRun, SplFrame, SplMeta};
use ac2_proto::model::{LeqJudgement, LevelScale, PeakQuantity};
use ac2_proto::units::Seconds;

use super::{Spl, q_index};
use crate::leq_log;

impl Spl {
    pub(super) fn spl_frame(&self) -> SplFrame {
        let mut l = self.meter.levels();
        let scale = match self.level_offset() {
            Some(offset_db) => {
                l = l.calibrated(Sensitivity { offset_db });
                // The peak takes the correction's peak difference, not the energy one.
                l.lpeak += self.peak_offset().unwrap_or(offset_db) - offset_db;
                LevelScale::DbSpl
            }
            None => LevelScale::Dbfs,
        };
        SplFrame {
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
                position: self.position(),
            },
        }
    }

    pub(super) fn leq_frame(&self) -> LeqFrame {
        let offset = self.level_offset();
        let o = offset.unwrap_or(0.0);
        let held = self.leq.peaks.max_dbfs();
        let peak = |q: PeakQuantity| {
            self.leq.cfg.peaks.get(q).map(|_| LeqPeak {
                level: held[q_index(q)] + self.offset_of(q).unwrap_or(0.0),
                judgement: self.leq.peak_judgements[q_index(q)],
            })
        };
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
                mic_curve: self.meter.has_correction(),
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
                lcpeak: peak(PeakQuantity::LcPeak),
                lafmax: peak(PeakQuantity::LafMax),
                position: self.position(),
            },
            leq: Vec::with_capacity(n),
            elapsed: Vec::with_capacity(n),
            measured: Vec::with_capacity(n),
            allowed: Vec::with_capacity(n),
            recover: Vec::with_capacity(n),
            least: Vec::with_capacity(n),
            over_in: Vec::with_capacity(n),
            flags: Vec::with_capacity(n),
        };
        for (i, w) in self.leq.cfg.windows.iter().enumerate() {
            let v = self.leq.ring.value(i);
            let mut flags = LeqFlags::NONE;
            let (mut allowed, mut recover, mut over_in) = (f32::NAN, f32::NAN, f32::NAN);
            if let Some(limit) = w.limit {
                flags = flags.with(LeqFlags::LIMIT);
                if offset.is_some() {
                    flags = flags.with(LeqFlags::JUDGED);
                    let p = ac2_core::leq::mean_square(limit.0 - o);
                    match self.leq.judgements.get(i) {
                        Some(LeqJudgement::Over) => flags = flags.with(LeqFlags::OVER),
                        Some(LeqJudgement::Near) => {
                            flags = flags.with(LeqFlags::NEAR);
                            if self.leq.on_course.get(i) == Some(&true) {
                                flags = flags.with(LeqFlags::ON_COURSE);
                                over_in = v.over_in(p).map_or(f32::NAN, |t| t as f32);
                            }
                        }
                        _ => {}
                    }
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
            f.least.push((v.least_dbfs() + o) as f32);
            f.over_in.push(over_in);
            f.flags.push(flags);
        }
        f
    }
}
