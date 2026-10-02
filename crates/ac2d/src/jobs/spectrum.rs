//! Narrowband spectrum job: tone level per FFT bin (`spec` frames) and input meters. A
//! calibrated input reads dB SPL; its mic curve, when on, is subtracted per bin.

use ac2_core::spectrum::{SpectrumAnalyzer, SpectrumConfig};
use ac2_proto::frame::{FrameData, ProtectionFlags, SpecFrame, SpecMeta, ValidityMask};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{LevelScale, SpectrumConfig as WireConfig};
use ac2_proto::units::{Hz, MeasId, Rev};

use super::{Analysis, Emitter, JobCmd, LevelsMeter, StampArgs, channel_f64};
use crate::calstore::InputCal;
use crate::conv;
use crate::fanout::Block;

pub(crate) struct Spectrum {
    meas: MeasId,
    cfg: WireConfig,
    idx: usize,
    analyzer: SpectrumAnalyzer,
    grid_id: GridId,
    cal: InputCal,
    /// Mic-curve correction per bin (dB subtracted).
    corr: Option<Vec<f64>>,
    frozen: bool,
    config_rev: Rev,
    applied_at: Option<u64>,
    levels: LevelsMeter,
    buf: Vec<f64>,
    end: Option<u64>,
    wall: u64,
}

/// The grid of a spectrum at `fs`.
pub(crate) fn grid(cfg: &WireConfig, fs: u32) -> GridDef {
    GridDef::Linear {
        fs: Hz(f64::from(fs)),
        n: cfg.fft_len,
    }
}

impl Spectrum {
    pub(crate) fn new(
        meas: MeasId,
        cfg: WireConfig,
        sample_rate: u32,
        idx: usize,
        cal: InputCal,
        frozen: bool,
        config_rev: Rev,
    ) -> Result<Self, String> {
        let n = cfg.fft_len as usize;
        let analyzer = SpectrumAnalyzer::new(SpectrumConfig {
            fs: f64::from(sample_rate),
            n,
            hop: (n / 2).max(1),
            window: conv::window(cfg.window),
            averaging: conv::spec_averaging(cfg.averaging).ok_or("invalid averaging")?,
            peak_hold: None,
        })
        .map_err(|e| e.to_string())?;
        Ok(Self {
            grid_id: grid(&cfg, sample_rate).id(),
            levels: LevelsMeter::new(vec![idx], vec![cfg.input], sample_rate),
            meas,
            cfg,
            idx,
            analyzer,
            corr: None,
            cal: InputCal::none(),
            frozen,
            config_rev,
            applied_at: None,
            buf: Vec::new(),
            end: None,
            wall: 0,
        }
        .with_cal(cal))
    }

    fn with_cal(mut self, cal: InputCal) -> Self {
        self.set_cal(cal);
        self
    }

    fn set_cal(&mut self, cal: InputCal) {
        let n = self.cfg.fft_len as usize;
        let df = self.analyzer.config().fs / n as f64;
        self.corr = cal
            .correction
            .as_ref()
            .map(|c| (0..=n / 2).map(|k| c.db(k as f64 * df)).collect());
        self.cal = cal;
    }
}

impl Analysis for Spectrum {
    fn push(&mut self, b: &Block) {
        self.applied_at.get_or_insert(b.start_sample);
        if self.end.is_some_and(|e| e != b.start_sample)
            || (self.end.is_some() && b.flags.breaks_continuity())
        {
            self.analyzer.reset_average();
        }
        if !self.frozen {
            channel_f64(b, self.idx, &mut self.buf);
            self.analyzer.push(&self.buf);
        }
        self.levels.push(b);
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
    }

    fn command(&mut self, c: JobCmd) {
        match c {
            JobCmd::Freeze(f) => self.frozen = f,
            JobCmd::Reset => self.analyzer.reset_average(),
            JobCmd::Cal(cal) => self.set_cal(*cal),
            JobCmd::SetDelay { .. } | JobCmd::Find { .. } | JobCmd::Track { .. } => {}
        }
    }

    fn emit(&mut self, e: &Emitter) {
        let Some(end) = self.end else {
            return;
        };
        let stamp = StampArgs {
            audio_sample: end.saturating_sub(1),
            config_rev: self.config_rev,
            applied_at: self.applied_at.unwrap_or(0),
            wall_ns: self.wall,
            grid_id: Some(self.grid_id),
            protection: if self.levels.any_clip_held() {
                ProtectionFlags::CLIP
            } else {
                ProtectionFlags::NONE
            },
        };
        if let Some(ps) = self.analyzer.average() {
            let off = self.cal.sensitivity.unwrap_or(0.0);
            let corr = self.corr.as_deref();
            let mut validity = Vec::with_capacity(ps.bins());
            let level: Vec<f32> = (0..ps.bins())
                .map(|k| {
                    let c = corr.and_then(|c| c.get(k)).copied().unwrap_or(0.0);
                    let v = ps.amplitude_dbfs(k) + off - c;
                    validity.push(if v.is_finite() {
                        ValidityMask::NONE
                    } else {
                        ValidityMask::BELOW_FLOOR
                    });
                    v as f32
                })
                .collect();
            e.send(
                stamp,
                FrameData::Spec(SpecFrame {
                    meas: self.meas,
                    meta: SpecMeta {
                        window: self.cfg.window,
                        scale: if self.cal.sensitivity.is_some() {
                            LevelScale::DbSpl
                        } else {
                            LevelScale::Dbfs
                        },
                        cal: self.cal.status,
                        mic_curve: self.corr.is_some(),
                    },
                    level,
                    validity,
                }),
            );
        }
        if let Some(l) = self.levels.take(self.meas) {
            e.send(
                StampArgs {
                    grid_id: None,
                    ..stamp
                },
                FrameData::Levels(l),
            );
        }
    }
}
