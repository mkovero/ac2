//! Narrowband spectrum job: tone level per FFT bin (`spec` frames) and input meters. A
//! calibrated input reads dB SPL; its mic curve, when on, is subtracted per bin. Display
//! smoothing, when set, power-averages the bins over a fractional-octave kernel; the
//! unsmoothed result is what a capture stores.

use ac2_core::smoothing::LinearSmoother;
use ac2_core::spectrum::{SpectrumAnalyzer, SpectrumConfig};
use ac2_proto::frame::{FrameData, ProtectionFlags, SpecFrame, SpecMeta, ValidityMask};
use ac2_proto::grid::{GridDef, GridId};
use ac2_proto::model::{LevelScale, SpectrumConfig as WireConfig};
use ac2_proto::units::{Hz, MeasId, Rev};

use super::{Analysis, Emitter, JobCmd, LevelsMeter, SmoothingChange, StampArgs, channel_f64};
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
    /// Display smoothing kernel for `cfg.smoothing`, built for the spectrum's bin count.
    smoother: Option<LinearSmoother>,
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

/// Updates per second the spectrum aims for. The FFT length sets frequency resolution; the
/// hop sets how often a new spectrum appears. Tying the hop to the length (n/2) made a
/// 65 536-point spectrum update only every ~0.7 s, so the hop is chosen for a fluid display
/// instead: heavily overlapped windows, about this many per second.
const SPECTRUM_UPDATES_PER_S: u32 = 30;

/// Hop for an `n`-point spectrum at `fs`: the largest power of two at or below
/// `fs / SPECTRUM_UPDATES_PER_S`, never more than half a window (overlap ≥ 50 %) and at least
/// 64 samples.
fn display_hop(n: usize, fs: u32) -> usize {
    let target = (fs / SPECTRUM_UPDATES_PER_S).max(64) as usize;
    let pow2 = 1usize << (usize::BITS - 1 - target.leading_zeros());
    pow2.min((n / 2).max(1)).max(1)
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
            hop: display_hop(n, sample_rate),
            window: conv::window(cfg.window),
            averaging: conv::spec_averaging(cfg.averaging).ok_or("invalid averaging")?,
            peak_hold: None,
        })
        .map_err(|e| e.to_string())?;
        let smoother = cfg
            .smoothing
            .map(|f| LinearSmoother::new(n / 2 + 1, conv::smoothing_fraction(f)));
        Ok(Self {
            grid_id: grid(&cfg, sample_rate).id(),
            levels: LevelsMeter::new(vec![idx], vec![cfg.input], sample_rate),
            meas,
            cfg,
            idx,
            analyzer,
            corr: None,
            smoother,
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
            JobCmd::Smoothing {
                change: SmoothingChange::Spectrum(smoothing),
                rev,
            } => {
                self.cfg.smoothing = smoothing;
                let bins = self.cfg.fft_len as usize / 2 + 1;
                self.smoother =
                    smoothing.map(|f| LinearSmoother::new(bins, conv::smoothing_fraction(f)));
                // Frames under the new rev carry the new setting from the next block on.
                self.config_rev = rev;
                self.applied_at = None;
            }
            JobCmd::SetDelay { .. }
            | JobCmd::Find { .. }
            | JobCmd::Track { .. }
            | JobCmd::Smoothing { .. }
            | JobCmd::Leq { .. } => {}
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
            let level: Vec<f32> = (0..ps.bins())
                .map(|k| {
                    let c = corr.and_then(|c| c.get(k)).copied().unwrap_or(0.0);
                    (ps.amplitude_dbfs(k) + off - c) as f32
                })
                .collect();
            let meta = SpecMeta {
                window: self.cfg.window,
                scale: if self.cal.sensitivity.is_some() {
                    LevelScale::DbSpl
                } else {
                    LevelScale::Dbfs
                },
                cal: self.cal.status,
                mic_curve: self.corr.is_some(),
                smoothing: self.cfg.smoothing,
            };
            let raw = SpecFrame {
                meas: self.meas,
                meta,
                validity: validity(&level),
                level,
            };
            match self
                .smoother
                .as_ref()
                .filter(|s| s.bins() == raw.level.len())
            {
                None => e.send(stamp, FrameData::Spec(raw)),
                Some(sm) => {
                    let level = smooth_levels(sm, &raw.level);
                    let shown = SpecFrame {
                        validity: validity(&level),
                        level,
                        ..raw.clone()
                    };
                    e.send_with_capture(stamp, FrameData::Spec(shown), FrameData::Spec(raw));
                }
            }
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

/// A bin without a finite level is below the analyser's floor.
fn validity(level: &[f32]) -> Vec<ValidityMask> {
    level
        .iter()
        .map(|v| {
            if v.is_finite() {
                ValidityMask::NONE
            } else {
                ValidityMask::BELOW_FLOOR
            }
        })
        .collect()
}

/// Tone levels (dB) smoothed as power. Bins below the floor are gaps, as they are in a
/// captured trace (whose columns hold them as NaN), so a capture re-smoothed at this
/// setting reads the same as this frame.
pub(crate) fn smooth_levels(sm: &LinearSmoother, level: &[f32]) -> Vec<f32> {
    let power: Vec<f64> = level
        .iter()
        .map(|l| 10f64.powf(f64::from(*l) / 10.0))
        .collect();
    let valid: Vec<bool> = level.iter().map(|l| l.is_finite()).collect();
    sm.smooth(&power, &valid)
        .iter()
        .zip(level)
        .map(|(p, l)| {
            if l.is_finite() {
                (10.0 * p.log10()) as f32
            } else {
                *l
            }
        })
        .collect()
}

#[cfg(test)]
mod hop_tests {
    use super::display_hop;

    #[test]
    fn hop_gives_a_fluid_update_rate_independent_of_fft_length() {
        // 48 kHz: 1024-sample hop ≈ 47 updates/s for every FFT length that allows it.
        assert_eq!(display_hop(65_536, 48_000), 1024);
        assert_eq!(display_hop(16_384, 48_000), 1024);
        // Short windows keep at least 50 % overlap.
        assert_eq!(display_hop(1024, 48_000), 512);
        assert_eq!(display_hop(256, 48_000), 128);
        // Other rates scale.
        assert_eq!(display_hop(65_536, 96_000), 2048);
        assert_eq!(display_hop(65_536, 44_100), 1024);
    }
}
