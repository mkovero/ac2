//! Sound level meter job (`spl` frames). Levels are dBFS, or dB SPL when the input has a
//! sensitivity calibration.

use ac2_core::spl::{Sensitivity, SplMeter, SplMeterConfig};
use ac2_proto::frame::{FrameData, ProtectionFlags, SplFrame, SplMeta};
use ac2_proto::model::{LevelScale, SplConfig};
use ac2_proto::units::{MeasId, Rev, Seconds};

use super::{Analysis, Emitter, JobCmd, LevelsMeter, StampArgs, channel_f64};
use crate::conv;
use crate::fanout::Block;

pub(crate) struct Spl {
    meas: MeasId,
    cfg: SplConfig,
    idx: usize,
    meter: SplMeter,
    sensitivity: Option<f64>,
    frozen: bool,
    config_rev: Rev,
    applied_at: Option<u64>,
    levels: LevelsMeter,
    buf: Vec<f64>,
    end: Option<u64>,
    wall: u64,
}

impl Spl {
    pub(crate) fn new(
        meas: MeasId,
        cfg: SplConfig,
        sample_rate: u32,
        idx: usize,
        sensitivity: Option<f64>,
        frozen: bool,
        config_rev: Rev,
    ) -> Result<Self, String> {
        let meter = SplMeter::new(SplMeterConfig {
            fs: f64::from(sample_rate),
            weighting: conv::weighting(cfg.weighting),
            time_weighting: conv::time_weighting(cfg.time_weighting),
            peak_weighting: conv::peak_weighting(cfg.peak_weighting),
        })
        .map_err(|e| e.to_string())?;
        Ok(Self {
            levels: LevelsMeter::new(vec![idx], vec![cfg.input], sample_rate),
            meas,
            cfg,
            idx,
            meter,
            sensitivity,
            frozen,
            config_rev,
            applied_at: None,
            buf: Vec::new(),
            end: None,
            wall: 0,
        })
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
            JobCmd::SetDelay { .. } | JobCmd::Find { .. } | JobCmd::Track { .. } => {}
        }
    }

    fn emit(&mut self, e: &Emitter) {
        let Some(end) = self.end else {
            return;
        };
        let mut l = self.meter.levels();
        let scale = match self.sensitivity {
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
                },
            }),
        );
        if let Some(lv) = self.levels.take(self.meas) {
            e.send(stamp, FrameData::Levels(lv));
        }
    }
}
