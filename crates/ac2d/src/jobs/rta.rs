//! Fractional-octave RTA job: IEC 61260-1 filterbank band power (`rta` frames).
//!
//! Each frame interval's band powers form one "frame" for averaging, which acts on power
//! (never on dB) like the spectrum's and weights each interval by its duration
//! ([`ac2_core::power_average`]). A calibrated input reads dB SPL; its mic curve, when
//! on, is subtracted per band as the curve's log-frequency power average over the band.

use ac2_core::power_average::PowerAverager;
use ac2_core::rta::OctaveFilterBank;
use ac2_core::spectrum::power_dbfs;
use ac2_core::weighting::WeightingFilter;
use ac2_proto::frame::{FrameData, ProtectionFlags, RtaFrame, RtaMeta, ValidityMask};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{LevelScale, RtaConfig};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{Hz, MeasId, Rev};

use super::{Analysis, Due, Emitter, Flush, JobCmd, LevelsMeter, Pace, StampArgs, channel_f64};
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

pub(crate) struct Rta {
    meas: MeasId,
    cfg: RtaConfig,
    idx: usize,
    fs: f64,
    weight: WeightingFilter,
    bank: OctaveFilterBank,
    grid_id: GridId,
    avg: PowerAverager,
    cal: InputCal,
    /// Mic-curve correction per band (dB subtracted).
    corr: Option<Vec<f64>>,
    config_rev: Rev,
    applied_at: Option<u64>,
    levels: LevelsMeter,
    buf: Vec<f64>,
    powers: Vec<f64>,
    shown: Option<Vec<f64>>,
    end: Option<u64>,
    wall: u64,
    /// Advances whenever the result may have changed: a new interval averaged, a reset, a
    /// command.
    generation: u64,
    pace: Pace,
}

impl Rta {
    pub(crate) fn new(
        meas: MeasId,
        cfg: RtaConfig,
        sample_rate: u32,
        idx: usize,
        cal: InputCal,
        config_rev: Rev,
    ) -> Result<Self, String> {
        let bank = bank(&cfg, sample_rate)?;
        let weight = WeightingFilter::new(conv::weighting(cfg.weighting), f64::from(sample_rate))
            .map_err(|e| e.to_string())?;
        let avg = conv::spec_averaging(cfg.averaging)
            .and_then(|a| PowerAverager::new(a, bank.len()).ok())
            .ok_or("invalid averaging")?;
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
            config_rev,
            applied_at: None,
            buf: Vec::new(),
            shown: None,
            end: None,
            wall: 0,
            generation: 0,
            pace: Pace::new(std::time::Duration::ZERO),
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
        self.avg.reset();
        self.shown = None;
        self.generation += 1;
    }

    fn stamp(&self, end: u64) -> StampArgs {
        StampArgs {
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
        }
    }

    /// The averaged band levels; `None` before the first interval.
    fn frame(&self) -> Option<RtaFrame> {
        let shown = self.shown.as_ref()?;
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
        Some(RtaFrame {
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
                math: None,
            },
            level,
            validity,
        })
    }
}

impl Analysis for Rta {
    fn result_generation(&self) -> Option<u64> {
        Some(self.generation)
    }

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
        self.generation += 1;
        match c {
            JobCmd::Reset => self.reset(),
            JobCmd::Cal(cal) => self.set_cal(*cal),
            JobCmd::SetDelay { .. }
            | JobCmd::Find { .. }
            | JobCmd::Track { .. }
            | JobCmd::Smoothing { .. }
            | JobCmd::Spl { .. } => {}
        }
    }

    fn emit(&mut self, e: &Emitter) -> Flush {
        let Some(end) = self.end else {
            return Flush::Done;
        };
        let interval_s = self.bank.samples() as f64 / self.fs;
        // Averaging goes on whether or not anyone receives the result: an interval is one
        // frame of the average.
        if self.bank.samples() > 0 {
            self.generation += 1;
            self.bank.band_powers(&mut self.powers);
            self.shown = Some(self.avg.push(&self.powers, interval_s).to_vec());
        }
        self.bank.reset_powers();
        let stamp = self.stamp(end);
        let topic = Topic::Data {
            meas: self.meas,
            stream: Stream::Rta,
        };
        let due = self
            .pace
            .due(e, topic, self.generation, &stamp, std::time::Instant::now());
        if due == Due::Send
            && let Some(f) = self.frame()
            && !e.send(stamp, FrameData::Rta(f))
        {
            self.pace.unsent();
        }
        self.levels.send(e, self.meas, stamp);
        Flush::from_due(due)
    }

    fn capture(&mut self) -> Option<(StampArgs, FrameData)> {
        let end = self.end?;
        Some((self.stamp(end), FrameData::Rta(self.frame()?)))
    }
}
