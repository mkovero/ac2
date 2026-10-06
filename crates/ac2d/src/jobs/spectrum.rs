//! Narrowband spectrum job: tone level per FFT bin and input meters. A calibrated input
//! reads dB SPL; its mic curve, when on, is subtracted per bin. Display smoothing, when set,
//! power-averages the bins over a fractional-octave kernel.
//!
//! The live `spec` frame gathers the bins into display columns ([`GridDef::LogBins`]), each
//! the highest level among its bins, so a 65 536-point spectrum is under a thousand columns
//! on the wire and a single-bin tone keeps its level. A capture takes every bin, unsmoothed,
//! on the FFT's own grid.

use ac2_core::smoothing::LinearSmoother;
use ac2_core::spectrum::{SpectrumAnalyzer, SpectrumConfig, column_max};
use ac2_proto::frame::{FrameData, ProtectionFlags, SpecFrame, SpecMeta};
use ac2_proto::grid::{BinColumns, GridDef, GridId};
use ac2_proto::model::{LevelScale, SpectrumConfig as WireConfig};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{Hz, MeasId, Rev};

use super::{
    Analysis, Due, Emitter, Flush, JobCmd, LevelsMeter, Pace, SmoothingChange, StampArgs,
    channel_f64,
};
use crate::calstore::InputCal;
use crate::conv;
use crate::fanout::Block;

/// Display columns per octave above the single-bin columns. The default 20 Hz – 20 kHz view
/// spans 10 octaves over roughly 1000–2000 pixels: 96 columns per octave is one every one
/// or two pixels, so the drawn line is the one every bin would give (the plot keeps the
/// highest point per pixel either way). Zoomed in further, or for an exact bin, a capture
/// has every bin.
pub(crate) const DISPLAY_PPO: u32 = 96;

pub(crate) struct Spectrum {
    meas: MeasId,
    cfg: WireConfig,
    idx: usize,
    analyzer: SpectrumAnalyzer,
    grid_id: GridId,
    capture_grid_id: GridId,
    /// First bin of each display column, then the bin count.
    first_bin: Vec<u32>,
    cal: InputCal,
    /// Per bin: folded power → tone power in the frame's scale (window gain, sensitivity
    /// and mic-curve correction as one factor).
    gain: Vec<f64>,
    /// Display smoothing kernel for `cfg.smoothing`, built for the spectrum's bin count.
    smoother: Option<LinearSmoother>,
    /// Tone power per bin.
    power: Vec<f64>,
    /// Smoothed tone power per bin (smoothing on).
    smoothed: Vec<f64>,
    /// Advances whenever there is something new to show: a spectrum, or a setting.
    generation: u64,
    /// When a `spec` frame goes out: on something new, and repeated while nothing changes
    /// (frozen, or a long hop) well inside the second after which a client calls it stale.
    pace: Pace,
    frozen: bool,
    config_rev: Rev,
    applied_at: Option<u64>,
    levels: LevelsMeter,
    buf: Vec<f64>,
    end: Option<u64>,
    wall: u64,
}

/// The live (display) grid of a spectrum at `fs`.
pub(crate) fn grid(cfg: &WireConfig, fs: u32) -> GridDef {
    GridDef::LogBins {
        fs: Hz(f64::from(fs)),
        n: cfg.fft_len,
        ppo: DISPLAY_PPO,
    }
}

/// The grid of a spectrum's capture: every FFT bin.
pub(crate) fn capture_grid(cfg: &WireConfig, fs: u32) -> GridDef {
    GridDef::Linear {
        fs: Hz(f64::from(fs)),
        n: cfg.fft_len,
    }
}

/// Updates per second the spectrum aims for when the window is short enough to allow it.
const SPECTRUM_UPDATES_PER_S: u32 = 30;

/// Hop for an `n`-point spectrum at `fs`: the largest power of two at or below
/// `fs / SPECTRUM_UPDATES_PER_S`, kept between `n / 8` and `n / 2` (overlap 50 – 87.5 %).
///
/// Frames closer than `n / 8` cost FFTs without adding information: the power estimates of
/// windows overlapping by more than ~75 % (Hann; the narrower flat-top and Blackman-Harris
/// windows by a little more) are almost fully correlated, so an average over a given time
/// is no steadier for having more of them. The exponential averaging's time constant is in
/// seconds and stays what it is. A 65 536-point window spans 1.37 s at 48 kHz; a new
/// spectrum every eighth of it (0.17 s) follows it closely, at an eighth of the FFT work a
/// 1024-sample hop costs. Short windows keep updating about 30 times a second.
fn display_hop(n: usize, fs: u32) -> usize {
    let target = (fs / SPECTRUM_UPDATES_PER_S).max(64) as usize;
    let pow2 = 1usize << (usize::BITS - 1 - target.leading_zeros());
    pow2.clamp((n / 8).max(1), (n / 2).max(1))
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
        let bins = analyzer.bins();
        let smoother = cfg
            .smoothing
            .map(|f| LinearSmoother::new(bins, conv::smoothing_fraction(f)));
        let columns = BinColumns::new(f64::from(sample_rate), cfg.fft_len, DISPLAY_PPO);
        Ok(Self {
            grid_id: grid(&cfg, sample_rate).id(),
            capture_grid_id: capture_grid(&cfg, sample_rate).id(),
            first_bin: columns.first_bin,
            levels: LevelsMeter::new(vec![idx], vec![cfg.input], sample_rate),
            meas,
            cfg,
            idx,
            analyzer,
            gain: Vec::new(),
            smoother,
            power: vec![0.0; bins],
            smoothed: Vec::new(),
            generation: 0,
            pace: Pace::new(std::time::Duration::ZERO),
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
        let off = cal.sensitivity.unwrap_or(0.0);
        let tone = self.analyzer.tone_power_factor();
        self.gain = (0..self.analyzer.bins())
            .map(|k| {
                let c = cal.correction.as_ref().map_or(0.0, |c| c.db(k as f64 * df));
                tone * 10f64.powf((off - c) / 10.0)
            })
            .collect();
        self.cal = cal;
        self.generation += 1;
    }
}

impl Spectrum {
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

    fn meta(&self) -> SpecMeta {
        SpecMeta {
            window: self.cfg.window,
            scale: if self.cal.sensitivity.is_some() {
                LevelScale::DbSpl
            } else {
                LevelScale::Dbfs
            },
            cal: self.cal.status,
            mic_curve: self.cal.correction.is_some(),
            smoothing: self.cfg.smoothing,
            math: None,
        }
    }

    /// Fills `power` with the averaged tone power per bin; `false` before the first
    /// spectrum. A bin without power is a gap (−∞ dB has no place on a dB axis), as it is in
    /// a captured trace; smoothing never crosses a gap, so a capture re-smoothed at this
    /// setting reads the same as the live frame.
    fn fill_power(&mut self) -> bool {
        let Some(ps) = self.analyzer.average() else {
            return false;
        };
        for ((p, f), g) in self.power.iter_mut().zip(ps.folded()).zip(&self.gain) {
            let x = f * g;
            *p = if x > 0.0 { x } else { f64::NAN };
        }
        true
    }
}

/// Tone power as a level; a bin without power (or a gap) has none.
fn db(p: f64) -> f32 {
    if p > 0.0 && p.is_finite() {
        (10.0 * p.log10()) as f32
    } else {
        f32::NAN
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
            if self.analyzer.push(&self.buf) > 0 {
                self.generation += 1;
            }
        }
        self.levels.push(b);
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
    }

    fn command(&mut self, c: JobCmd) {
        match c {
            JobCmd::Freeze(f) => {
                self.frozen = f;
                self.generation += 1;
            }
            JobCmd::Reset => self.analyzer.reset_average(),
            JobCmd::Cal(cal) => self.set_cal(*cal),
            JobCmd::Smoothing {
                change: SmoothingChange::Spectrum(smoothing),
                rev,
            } => {
                self.cfg.smoothing = smoothing;
                let bins = self.analyzer.bins();
                self.smoother =
                    smoothing.map(|f| LinearSmoother::new(bins, conv::smoothing_fraction(f)));
                // Frames under the new rev carry the new setting from the next block on.
                self.config_rev = rev;
                self.applied_at = None;
                self.generation += 1;
            }
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
        let stamp = self.stamp(end);
        let topic = Topic::Data {
            meas: self.meas,
            stream: Stream::Spec,
        };
        let due = self
            .pace
            .due(e, topic, self.generation, &stamp, std::time::Instant::now());
        if due == Due::Send && self.fill_power() {
            let bins = self.power.len();
            let shown = match self.smoother.as_mut().filter(|s| s.bins() == bins) {
                Some(sm) => {
                    self.smoothed.resize(bins, 0.0);
                    sm.smooth_into(&self.power, &mut self.smoothed);
                    &self.smoothed
                }
                None => &self.power,
            };
            let live = SpecFrame {
                meas: self.meas,
                meta: self.meta(),
                level: column_max(shown, &self.first_bin).map(db).collect(),
            };
            if !e.send(stamp, FrameData::Spec(live)) {
                self.pace.unsent();
            }
        }
        self.levels.send(e, self.meas, stamp);
        Flush::from_due(due)
    }

    fn capture(&mut self) -> Option<(StampArgs, FrameData)> {
        let end = self.end?;
        if !self.fill_power() {
            return None;
        }
        let capture = SpecFrame {
            meas: self.meas,
            meta: self.meta(),
            level: self.power.iter().map(|p| db(*p)).collect(),
        };
        let stamp = StampArgs {
            grid_id: Some(self.capture_grid_id),
            ..self.stamp(end)
        };
        Some((stamp, FrameData::Spec(capture)))
    }
}

#[cfg(test)]
mod hop_tests {
    use super::display_hop;

    #[test]
    fn hop_overlaps_between_half_and_seven_eighths() {
        // 48 kHz: long windows step by n/8 (87.5 % overlap) …
        assert_eq!(display_hop(65_536, 48_000), 8192);
        assert_eq!(display_hop(16_384, 48_000), 2048);
        // … short ones by a 1024-sample hop (~47 updates/s) while that is at most n/2.
        assert_eq!(display_hop(4096, 48_000), 1024);
        assert_eq!(display_hop(1024, 48_000), 512);
        assert_eq!(display_hop(256, 48_000), 128);
        // Other rates scale.
        assert_eq!(display_hop(4096, 96_000), 2048);
        assert_eq!(display_hop(65_536, 44_100), 8192);
    }
}
