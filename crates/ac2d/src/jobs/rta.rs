//! Fractional-octave RTA job: IEC 61260-1 filterbank band power (`rta` frames).
//!
//! Each frame interval's band powers form one "frame" for averaging, which acts on power
//! (never on dB) like the spectrum's. A calibrated input reads dB SPL; its mic curve, when
//! on, is subtracted per band as the curve's log-frequency power average over the band.

use std::collections::VecDeque;

use ac2_core::rta::OctaveFilterBank;
use ac2_core::spectrum::power_dbfs;
use ac2_core::weighting::WeightingFilter;
use ac2_proto::frame::{FrameData, ProtectionFlags, RtaFrame, RtaMeta, ValidityMask};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{LevelScale, RtaConfig, SpecAveraging};
use ac2_proto::units::{Hz, MeasId, Rev};

use super::{Analysis, Emitter, JobCmd, LevelsMeter, StampArgs, channel_f64};
use crate::calstore::InputCal;
use crate::conv;
use crate::fanout::Block;

/// The filterbank of `cfg` at `fs`.
pub(crate) fn bank(cfg: &RtaConfig, fs: u32) -> Result<OctaveFilterBank, String> {
    if !(cfg.f_lo.0.is_finite()
        && cfg.f_hi.0.is_finite()
        && cfg.f_lo.0 > 0.0
        && cfg.f_hi.0 > cfg.f_lo.0)
    {
        return Err("f_lo must be positive and below f_hi".into());
    }
    OctaveFilterBank::new(
        conv::band_fraction(cfg.fraction),
        f64::from(fs),
        cfg.f_lo.0,
        cfg.f_hi.0,
    )
    .map_err(|e| e.to_string())
}

/// The grid of `bank`.
pub(crate) fn grid(cfg: &RtaConfig, bank: &OctaveFilterBank) -> GridDef {
    GridDef::IecBands {
        fraction: cfg.fraction,
        centres: bank.bands().map(|b| Hz(b.centre_hz)).collect(),
    }
}

enum Avg {
    Off,
    Fifo(usize, VecDeque<Vec<f64>>),
    Exp(f64, Option<Vec<f64>>),
}

pub(crate) struct Rta {
    meas: MeasId,
    cfg: RtaConfig,
    idx: usize,
    fs: f64,
    weight: WeightingFilter,
    bank: OctaveFilterBank,
    grid_id: GridId,
    avg: Avg,
    cal: InputCal,
    /// Mic-curve correction per band (dB subtracted).
    corr: Option<Vec<f64>>,
    frozen: bool,
    config_rev: Rev,
    applied_at: Option<u64>,
    levels: LevelsMeter,
    buf: Vec<f64>,
    powers: Vec<f64>,
    shown: Option<Vec<f64>>,
    end: Option<u64>,
    wall: u64,
}

impl Rta {
    pub(crate) fn new(
        meas: MeasId,
        cfg: RtaConfig,
        sample_rate: u32,
        idx: usize,
        cal: InputCal,
        frozen: bool,
        config_rev: Rev,
    ) -> Result<Self, String> {
        let bank = bank(&cfg, sample_rate)?;
        let weight = WeightingFilter::new(conv::weighting(cfg.weighting), f64::from(sample_rate))
            .map_err(|e| e.to_string())?;
        let avg = match cfg.averaging {
            SpecAveraging::Off => Avg::Off,
            SpecAveraging::Fifo { frames } if frames >= 1 => {
                Avg::Fifo(frames as usize, VecDeque::new())
            }
            SpecAveraging::Exponential { time_constant }
                if time_constant.0.is_finite() && time_constant.0 > 0.0 =>
            {
                Avg::Exp(time_constant.0, None)
            }
            _ => return Err("invalid averaging".into()),
        };
        let mut r = Self {
            grid_id: grid(&cfg, &bank).id(),
            powers: vec![0.0; bank.len()],
            levels: LevelsMeter::new(vec![idx], vec![cfg.input], sample_rate),
            meas,
            cfg,
            idx,
            fs: f64::from(sample_rate),
            weight,
            bank,
            avg,
            cal: InputCal::none(),
            corr: None,
            frozen,
            config_rev,
            applied_at: None,
            buf: Vec::new(),
            shown: None,
            end: None,
            wall: 0,
        };
        r.set_cal(cal);
        Ok(r)
    }

    fn set_cal(&mut self, cal: InputCal) {
        self.corr = cal.correction.as_ref().map(|c| {
            self.bank
                .bands()
                .map(|b| c.band_db(b.lower_hz, b.upper_hz))
                .collect()
        });
        self.cal = cal;
    }

    fn reset(&mut self) {
        match &mut self.avg {
            Avg::Off => {}
            Avg::Fifo(_, q) => q.clear(),
            Avg::Exp(_, s) => *s = None,
        }
        self.shown = None;
    }
}

impl Analysis for Rta {
    fn push(&mut self, b: &Block) {
        self.applied_at.get_or_insert(b.start_sample);
        if self.end.is_some_and(|e| e != b.start_sample)
            || (self.end.is_some() && b.flags.breaks_continuity())
        {
            self.bank.reset();
            self.weight.reset();
            self.reset();
        }
        channel_f64(b, self.idx, &mut self.buf);
        self.weight.process(&mut self.buf);
        self.bank.process(&self.buf);
        self.levels.push(b);
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
    }

    fn command(&mut self, c: JobCmd) {
        match c {
            JobCmd::Freeze(f) => self.frozen = f,
            JobCmd::Reset => self.reset(),
            JobCmd::Cal(cal) => self.set_cal(*cal),
            JobCmd::SetDelay { .. }
            | JobCmd::Find { .. }
            | JobCmd::Track { .. }
            | JobCmd::Smoothing { .. }
            | JobCmd::Spl { .. } => {}
        }
    }

    fn emit(&mut self, e: &Emitter) {
        let Some(end) = self.end else {
            return;
        };
        let interval_s = self.bank.samples() as f64 / self.fs;
        if self.bank.samples() > 0 && !self.frozen {
            self.bank.band_powers(&mut self.powers);
            let p = self.powers.clone();
            self.shown = Some(match &mut self.avg {
                Avg::Off => p,
                Avg::Fifo(n, q) => {
                    q.push_back(p);
                    while q.len() > *n {
                        q.pop_front();
                    }
                    let k = q.len() as f64;
                    (0..self.powers.len())
                        .map(|i| q.iter().map(|v| v[i]).sum::<f64>() / k)
                        .collect()
                }
                Avg::Exp(tau, s) => {
                    let a = 1.0 - (-interval_s / *tau).exp();
                    match s {
                        None => {
                            *s = Some(p.clone());
                            p
                        }
                        Some(prev) => {
                            for (x, y) in prev.iter_mut().zip(&p) {
                                *x += a * (y - *x);
                            }
                            prev.clone()
                        }
                    }
                }
            });
        }
        self.bank.reset_powers();
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
        if let Some(shown) = &self.shown {
            let off = self.cal.sensitivity.unwrap_or(0.0);
            let corr = self.corr.as_deref();
            let level: Vec<f32> = shown
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let c = corr.and_then(|c| c.get(i)).copied().unwrap_or(0.0);
                    (power_dbfs(*p) + off - c) as f32
                })
                .collect();
            let validity = level
                .iter()
                .map(|v| {
                    if v.is_finite() {
                        ValidityMask::NONE
                    } else {
                        ValidityMask::BELOW_FLOOR
                    }
                })
                .collect();
            e.send(
                stamp,
                FrameData::Rta(RtaFrame {
                    meas: self.meas,
                    meta: RtaMeta {
                        fraction: self.cfg.fraction,
                        weighting: self.cfg.weighting,
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
