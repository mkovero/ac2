//! Sound level meter job (`spl` frames). Levels are dBFS, or dB SPL when a sensitivity
//! calibration applies to the input; the input's mic curve, when on, runs as a
//! minimum-phase filter before frequency weighting (`docs/design/q7-calibration.md` §6).

use std::sync::Arc;

use ac2_core::mic_curve::Correction;
use ac2_core::spl::{Sensitivity, SplMeter, SplMeterConfig};
use ac2_proto::frame::{FrameData, ProtectionFlags, SplFrame, SplMeta};
use ac2_proto::model::{LevelScale, SplConfig};
use ac2_proto::units::{MeasId, Rev, Seconds};

use super::{Analysis, Emitter, JobCmd, LevelsMeter, StampArgs, channel_f64};
use crate::calstore::InputCal;
use crate::conv;
use crate::fanout::Block;

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
}

fn same_curve(a: Option<&Arc<Correction>>, b: Option<&Arc<Correction>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
        _ => false,
    }
}

impl Spl {
    pub(crate) fn new(
        meas: MeasId,
        cfg: SplConfig,
        sample_rate: u32,
        idx: usize,
        cal: InputCal,
        frozen: bool,
        config_rev: Rev,
    ) -> Result<Self, String> {
        let fs = f64::from(sample_rate);
        let mut meter = SplMeter::new(SplMeterConfig {
            fs,
            weighting: conv::weighting(cfg.weighting),
            time_weighting: conv::time_weighting(cfg.time_weighting),
            peak_weighting: conv::peak_weighting(cfg.peak_weighting),
        })
        .map_err(|e| e.to_string())?;
        if let Some(c) = &cal.correction {
            meter.set_correction(Some(&c.design_fir(fs)));
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
        })
    }

    fn set_cal(&mut self, cal: InputCal) {
        if !same_curve(self.cal.correction.as_ref(), cal.correction.as_ref()) {
            let taps = cal.correction.as_ref().map(|c| c.design_fir(self.fs));
            self.meter.set_correction(taps.as_deref());
        }
        self.cal = cal;
    }
}

impl Analysis for Spl {
    fn push(&mut self, b: &Block) {
        self.applied_at.get_or_insert(b.start_sample);
        if !self.frozen {
            channel_f64(b, self.idx, &mut self.buf);
            self.meter.process(&self.buf);
        }
        self.levels.push(b);
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
    }

    fn command(&mut self, c: JobCmd) {
        match c {
            JobCmd::Freeze(f) => self.frozen = f,
            JobCmd::Reset => self.meter.reset_interval(),
            JobCmd::Cal(cal) => self.set_cal(*cal),
            JobCmd::SetDelay { .. } | JobCmd::Find { .. } | JobCmd::Track { .. } => {}
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
        if let Some(lv) = self.levels.take(self.meas) {
            e.send(stamp, FrameData::Levels(lv));
        }
    }
}
