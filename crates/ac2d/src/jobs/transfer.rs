//! Transfer-function job: protection → MTW ladder → optional smoothing → `tf` frames, plus
//! the live IR view (only while someone subscribes to it) and input meters. The measurement
//! input's mic curve, when on, is subtracted from the displayed magnitude (never the phase,
//! the IR or the delay finder: decision 7c).

use ac2_core::grid::LogGrid;
use ac2_core::ir_view::{ImpulseResponse, UniformTf};
use ac2_core::mic_curve::Correction;
use ac2_core::mtw::{Ladder, Mtw, MtwConfig, SampleGate, Validity};
use ac2_core::protection::{BlockDecision, BlockLevels, Guard, ProtectionConfig};
use ac2_core::smoothing::{Smoother, SmoothingMode, TfColumns};
use ac2_proto::frame::{
    FrameData, IrFrame, IrMeta, ProtectionFlags, TfFrame, TfMeta, ValidityMask,
};
use ac2_proto::grid::GridId;
use ac2_proto::model::TransferConfig;
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{Hz, MeasId, Rev, Seconds};
use num_complex::Complex64;

use std::sync::mpsc::Sender;

use ac2_proto::units::SessionEpoch;

use super::finder::Finder;
use super::{Analysis, Emitter, JobCmd, LevelsMeter, StampArgs};
use crate::control::ControlMsg;
use crate::conv;
use crate::fanout::Block;

/// Zero-lag correlation above which reference and measurement are taken to be the same
/// signal. An acoustic path always delays the measurement, and broadband programme or noise
/// decorrelates within a few samples, so this only happens when one source is patched to
/// both inputs.
const ROUTING_CORRELATION: f64 = 0.99;
/// Measurement level above which a silent reference means swapped inputs, dBFS.
const ROUTING_SIGNAL_DBFS: f64 = -60.0;

/// Zero-lag routing check over one frame interval.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RoutingCheck {
    rr: f64,
    mm: f64,
    rm: f64,
    n: u64,
}

impl RoutingCheck {
    pub(crate) fn push(&mut self, r: &[f32], m: &[f32]) {
        for (a, b) in r.iter().zip(m) {
            let (a, b) = (f64::from(*a), f64::from(*b));
            self.rr += a * a;
            self.mm += b * b;
            self.rm += a * b;
        }
        self.n += r.len().min(m.len()) as u64;
    }

    /// Whether the interval looks mis-patched, given the reference floor; then a new interval.
    pub(crate) fn take(&mut self, reference_floor_dbfs: f64) -> bool {
        let c = *self;
        *self = Self::default();
        if c.n == 0 {
            return false;
        }
        let n = c.n as f64;
        let ref_db = ac2_core::spectrum::rms_dbfs((c.rr / n).sqrt());
        let meas_db = ac2_core::spectrum::rms_dbfs((c.mm / n).sqrt());
        if ref_db < reference_floor_dbfs && meas_db > ROUTING_SIGNAL_DBFS {
            return true;
        }
        if ref_db >= reference_floor_dbfs && c.mm > 0.0 {
            let corr = c.rm / (c.rr * c.mm).sqrt();
            return corr.abs() >= ROUTING_CORRELATION;
        }
        false
    }
}

pub(crate) struct Transfer {
    meas: MeasId,
    cfg: TransferConfig,
    fs: f64,
    ref_idx: usize,
    meas_idx: usize,
    mtw: Mtw,
    guard: Guard,
    smoother: Option<(Smoother, SmoothingMode)>,
    grid_id: GridId,
    delay_s: f64,
    delay_samples: i64,
    frozen: bool,
    config_rev: Rev,
    applied_at: u64,
    apply_pending: bool,
    levels: LevelsMeter,
    routing: RoutingCheck,
    check_routing: bool,
    rbuf: Vec<f32>,
    mbuf: Vec<f32>,
    end: Option<u64>,
    wall: u64,
    discontinuity_until: u64,
    weak_until: u64,
    finder: Finder,
    epoch: SessionEpoch,
    to_control: Sender<ControlMsg>,
    grid: LogGrid,
    /// Mic-curve correction per column (dB subtracted from `mag`).
    corr: Option<Vec<f64>>,
}

/// The mic-curve correction at each column of `grid`.
fn column_correction(grid: &LogGrid, c: &Correction) -> Vec<f64> {
    grid.frequencies().into_iter().map(|f| c.db(f)).collect()
}

/// Why a transfer job cannot start.
pub(crate) type StartError = String;

impl Transfer {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        meas: MeasId,
        cfg: TransferConfig,
        sample_rate: u32,
        ref_idx: usize,
        meas_idx: usize,
        delay_samples: i64,
        delay_s: f64,
        frozen: bool,
        config_rev: Rev,
        tracking: bool,
        awaiting_pick: bool,
        epoch: SessionEpoch,
        to_control: Sender<ControlMsg>,
        correction: Option<&Correction>,
    ) -> Result<Self, StartError> {
        let fs = f64::from(sample_rate);
        let grid = LogGrid {
            ppo: cfg.grid.ppo,
            k_min: cfg.grid.k_min,
            k_max: cfg.grid.k_max,
        };
        let averaging = conv::tf_averaging(cfg.averaging).ok_or("invalid averaging")?;
        let mut mtw = Mtw::new(MtwConfig {
            sample_rate_hz: fs,
            ladder: Ladder::Standard,
            depth: conv::depth(cfg.depth).ok_or("invalid depth policy")?,
            averaging,
            grid,
            delay_samples,
        })
        .map_err(|e| e.to_string())?;
        mtw.set_frozen(frozen);
        let smoother = cfg.smoothing.map(|s| {
            let (f, m) = conv::smoothing(s);
            (Smoother::new(grid, f), m)
        });
        let grid_id = ac2_proto::grid::GridDef::Log {
            ppo: grid.ppo,
            k_min: grid.k_min,
            k_max: grid.k_max,
        }
        .id();
        let mut finder = Finder::new(fs);
        finder.track(tracking, delay_samples);
        finder.set_paused(awaiting_pick);
        Ok(Self {
            corr: correction.map(|c| column_correction(&grid, c)),
            grid,
            finder,
            epoch,
            to_control,
            levels: LevelsMeter::new(
                vec![ref_idx, meas_idx],
                vec![cfg.reference_input, cfg.measurement_input],
                sample_rate,
            ),
            meas,
            cfg,
            fs,
            ref_idx,
            meas_idx,
            mtw,
            guard: Guard::new(ProtectionConfig::default()),
            smoother,
            grid_id,
            delay_s,
            delay_samples,
            frozen,
            config_rev,
            applied_at: 0,
            apply_pending: true,
            routing: RoutingCheck::default(),
            check_routing: false,
            rbuf: Vec::new(),
            mbuf: Vec::new(),
            end: None,
            wall: 0,
            discontinuity_until: 0,
            weak_until: 0,
        })
    }

    fn set_correction(&mut self, c: Option<&Correction>) {
        self.corr = c.map(|c| column_correction(&self.grid, c));
    }

    fn mark_discontinuity(&mut self, at: u64) {
        // The flag stays up for a second so a client sees the restart.
        self.discontinuity_until = at + self.fs as u64;
    }

    fn ir_frame(&self) -> Option<IrFrame> {
        let st = self.mtw.stage_spectra(0)?;
        let h: Vec<Complex64> = st
            .gxy
            .iter()
            .zip(&st.gxx)
            .map(|(xy, xx)| {
                if *xx > 0.0 {
                    *xy / *xx
                } else {
                    Complex64::new(0.0, 0.0)
                }
            })
            .collect();
        let ir = ImpulseResponse::from_tf(UniformTf {
            h: &h,
            sample_rate: self.fs,
            inserted_delay_s: self.delay_s,
        })
        .ok()?;
        Some(IrFrame {
            meas: self.meas,
            meta: IrMeta {
                sample_rate: Hz(self.fs),
                t0: Seconds(ir.time_s(0)),
                dt: Seconds(1.0 / self.fs),
                inserted_delay: Seconds(self.delay_s),
            },
            linear: ir.linear().iter().map(|v| *v as f32).collect(),
            etc: Some(ir.etc_db().into_iter().map(|v| v as f32).collect()),
        })
    }
}

impl Analysis for Transfer {
    fn push(&mut self, b: &Block) {
        let contiguous = self.end == Some(b.start_sample);
        if self.end.is_some() && (!contiguous || b.flags.breaks_continuity()) {
            if contiguous {
                // An xrun or overflow can leave the counter contiguous while audio was lost;
                // the block grid must restart all the same.
                self.mtw.set_delay(self.delay_samples);
            }
            self.mark_discontinuity(b.start_sample);
            self.finder.restart();
        }
        if self.apply_pending {
            self.applied_at = b.start_sample;
            self.apply_pending = false;
        }
        b.channel_into(self.ref_idx, &mut self.rbuf);
        b.channel_into(self.meas_idx, &mut self.mbuf);
        let lv = BlockLevels::from_samples(&self.rbuf, &self.mbuf, self.fs);
        let gate = match self.guard.process(&lv) {
            BlockDecision::Accept => SampleGate::Accept,
            BlockDecision::HoldWeakReference => {
                self.weak_until = b.end_sample() + (self.fs / 2.0) as u64;
                SampleGate::Reject
            }
            BlockDecision::RejectClip | BlockDecision::PauseNoReference => SampleGate::Reject,
        };
        if let Err(e) = self.mtw.push(b.start_sample, &self.rbuf, &self.mbuf, gate) {
            tracing::error!("transfer {}: {e}", self.meas);
        }
        if let Some(d) = self.finder.push(b.start_sample, &self.rbuf, &self.mbuf) {
            tracing::info!(
                "measurement {}: tracking moves the delay to {d} samples",
                self.meas.0
            );
            let _ = self.to_control.send(ControlMsg::DelayTracked {
                meas: self.meas,
                epoch: self.epoch,
                samples: d,
            });
        }
        self.routing.push(&self.rbuf, &self.mbuf);
        self.levels.push(b);
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
    }

    fn command(&mut self, c: JobCmd) {
        match c {
            JobCmd::SetDelay {
                samples,
                seconds,
                rev,
                resume,
            } => {
                self.delay_samples = samples;
                self.delay_s = seconds;
                self.config_rev = rev;
                self.apply_pending = true;
                self.mtw.set_delay(samples);
                self.finder.set_held(samples);
                if resume {
                    self.finder.set_paused(false);
                }
            }
            JobCmd::Find {
                token,
                band,
                observation,
            } => {
                let result = self.finder.find(band, observation, self.delay_samples);
                let _ = self.to_control.send(ControlMsg::DelayFound {
                    token,
                    result: Box::new(result),
                });
            }
            JobCmd::Track { enabled } => {
                self.finder.track(enabled, self.delay_samples);
            }
            JobCmd::Freeze(f) => {
                self.frozen = f;
                self.mtw.set_frozen(f);
            }
            JobCmd::Reset => self.mtw.reset_averages(),
            JobCmd::Cal(cal) => self.set_correction(cal.correction.as_deref()),
        }
    }

    fn emit(&mut self, e: &Emitter) {
        let Some(end) = self.end else {
            return;
        };
        let routing = self.routing.take(self.guard.config().reference_floor_dbfs);
        self.check_routing = routing;
        let banners = self.guard.banners();
        let mut prot = ProtectionFlags::NONE;
        if banners.no_reference {
            prot = prot.with(ProtectionFlags::NO_REFERENCE);
        }
        if banners.no_signal {
            prot = prot.with(ProtectionFlags::NO_SIGNAL);
        }
        if banners.clip || self.levels.any_clip_held() {
            prot = prot.with(ProtectionFlags::CLIP);
        }
        if self.weak_until > end {
            prot = prot.with(ProtectionFlags::WEAK_REFERENCE);
        }
        if self.discontinuity_until > end {
            prot = prot.with(ProtectionFlags::DISCONTINUITY);
        }
        if self.check_routing {
            prot = prot.with(ProtectionFlags::CHECK_ROUTING);
        }
        let stamp = StampArgs {
            audio_sample: end.saturating_sub(1),
            config_rev: self.config_rev,
            applied_at: self.applied_at,
            wall_ns: self.wall,
            grid_id: Some(self.grid_id),
            protection: prot,
        };

        let f = self.mtw.frame();
        let validity: Vec<ValidityMask> = f
            .columns
            .iter()
            .map(|c| match c.validity {
                Validity::Valid => ValidityMask::NONE,
                Validity::Thinned => ValidityMask::THINNED,
                Validity::OutOfBand => ValidityMask::OUT_OF_BAND,
                Validity::Settling => ValidityMask::SETTLING,
                Validity::NoReference => ValidityMask::NO_REFERENCE,
                Validity::NoMeasurement => ValidityMask::NO_MEASUREMENT,
            })
            .collect();
        let (mut mag, phase): (Vec<f32>, Vec<f32>) = match &self.smoother {
            None => (
                f.magnitude_db.iter().map(|v| *v as f32).collect(),
                f.phase_deg.iter().map(|v| *v as f32).collect(),
            ),
            Some((sm, mode)) => {
                let valid: Vec<bool> = validity.iter().map(|v| *v == ValidityMask::NONE).collect();
                let s = sm.smooth(
                    TfColumns {
                        h: &f.h1,
                        coherence: &f.coherence,
                        valid: &valid,
                    },
                    *mode,
                );
                s.h.iter()
                    .zip(&s.valid)
                    .map(|(h, ok)| {
                        if *ok {
                            (
                                (20.0 * h.norm().log10()) as f32,
                                h.arg().to_degrees() as f32,
                            )
                        } else {
                            (f32::NAN, f32::NAN)
                        }
                    })
                    .unzip()
            }
        };
        if let Some(c) = &self.corr {
            for (m, d) in mag.iter_mut().zip(c) {
                *m -= *d as f32;
            }
        }
        e.send(
            stamp,
            FrameData::Tf(TfFrame {
                meas: self.meas,
                meta: TfMeta {
                    delay: Seconds(self.delay_s),
                    frozen: self.frozen,
                    smoothing: self.cfg.smoothing,
                    mic_curve: self.corr.is_some(),
                },
                mag,
                phase,
                coh: f.coherence.iter().map(|v| *v as f32).collect(),
                eff_avg: Some(f.eff_avg.iter().map(|v| *v as f32).collect()),
                validity,
            }),
        );

        if e.wants(Topic::Data {
            meas: self.meas,
            stream: Stream::Ir,
        }) && let Some(ir) = self.ir_frame()
        {
            e.send(
                StampArgs {
                    grid_id: None,
                    ..stamp
                },
                FrameData::Ir(ir),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_check() {
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let noise: Vec<f32> = (0..4800)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                ((x >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.2
            })
            .collect();
        let mut c = RoutingCheck::default();
        // Same signal on both inputs, different gain.
        let half: Vec<f32> = noise.iter().map(|v| v * 0.5).collect();
        c.push(&noise, &half);
        assert!(c.take(-80.0));
        // Reference silent, measurement live.
        c.push(&vec![0.0; 4800], &noise);
        assert!(c.take(-80.0));
        // Delayed copy: not flagged.
        let delayed: Vec<f32> = std::iter::repeat_n(0.0, 37)
            .chain(noise.iter().copied())
            .take(4800)
            .collect();
        c.push(&noise, &delayed);
        assert!(!c.take(-80.0));
        // Nothing at all: not flagged.
        c.push(&vec![0.0; 4800], &vec![0.0; 4800]);
        assert!(!c.take(-80.0));
    }
}
