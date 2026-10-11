//! Narrowband spectrum job: tone level per FFT bin and input meters. A calibrated input
//! reads dB SPL; its mic curve, when on, is subtracted per bin. Display smoothing, when set,
//! power-averages the bins over a fractional-octave kernel.
//!
//! The live `spec` frame gathers the bins into display columns ([`GridDef::LogBins`]), each
//! the highest level among its bins, so a 65 536-point spectrum is under a thousand columns
//! on the wire and a single-bin tone keeps its level. A capture takes every bin, unsmoothed,
//! on the FFT's own grid.
//!
//! A multi-resolution spectrum ([`SpecResolution::Multi`], `ac2_core::multires`) has
//! log-spaced columns on the live frame and in a capture alike, each the tone level its
//! bands give; the frame names the bands, how old each one's newest spectrum is and where
//! the columns are finer than the band behind them resolves.

use ac2_core::grid::LogGrid;
use ac2_core::multires::{MultiResAnalyzer, MultiResConfig, MultiResLadder};
use ac2_core::smoothing::{LinearSmoother, Smoother, SmoothingMode, TfColumns};
use ac2_core::spectrum::{SpectrumAnalyzer, SpectrumConfig, column_max};
use ac2_proto::frame::{FrameData, ProtectionFlags, SpecBand, SpecBands, SpecFrame, SpecMeta};
use ac2_proto::grid::{BinColumns, GridDef, GridId};
use ac2_proto::model::{
    FreqRange, LevelScale, Resolution, SmoothingFraction, SpecResolution,
    SpectrumConfig as WireConfig, Unresolved,
};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{Hz, MeasId, Rev, Seconds};
use num_complex::Complex64;

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
    engine: Engine,
    grid_id: GridId,
    capture_grid_id: GridId,
    cal: InputCal,
    /// Advances whenever there is something new to show: a spectrum, or a setting.
    generation: u64,
    /// When a `spec` frame goes out: on something new, and repeated while nothing changes
    /// (a long hop) well inside the second after which a client calls it stale.
    pace: Pace,
    config_rev: Rev,
    applied_at: Option<u64>,
    levels: LevelsMeter,
    buf: Vec<f64>,
    end: Option<u64>,
    wall: u64,
    /// End (exclusive) and wall clock of the newest analysed frame. A new result is
    /// stamped here rather than at the newest block: audio is handed on in batches whose
    /// period beats against the hop, so the newest block lands anywhere up to a hand-off
    /// after the frame, and a stamp there would make evenly spaced spectra look unevenly
    /// spaced in time to whatever places them by it (a spectrograph, a display glide).
    result: Option<(u64, u64)>,
    /// `result` has not gone out in a frame yet.
    fresh: bool,
}

/// The analysis behind a spectrum.
enum Engine {
    Fixed(Box<Fixed>),
    Multi(Box<Multi>),
}

/// One FFT on linear bins.
struct Fixed {
    analyzer: SpectrumAnalyzer,
    /// First bin of each display column, then the bin count.
    first_bin: Vec<u32>,
    /// Per bin: folded power → tone power in the frame's scale (window gain, sensitivity
    /// and mic-curve correction as one factor).
    gain: Vec<f64>,
    /// Display smoothing kernel for `cfg.smoothing`, built for the spectrum's bin count.
    smoother: Option<LinearSmoother>,
    /// Tone power per bin.
    power: Vec<f64>,
    /// Smoothed tone power per bin (smoothing on).
    smoothed: Vec<f64>,
}

/// The multi-resolution bands on log columns.
struct Multi {
    analyzer: MultiResAnalyzer,
    grid: LogGrid,
    resolution: Resolution,
    /// Display smoothing on the columns (power, the kernel stored traces are smoothed with).
    smoother: Option<Smoother>,
    /// Tone power per column.
    power: Vec<f64>,
    /// Where the columns are finer than the band behind them resolves.
    unresolved: Vec<FreqRange>,
}

impl Multi {
    fn set_smoothing(&mut self, f: Option<SmoothingFraction>) {
        self.smoother = f.map(|f| Smoother::new(self.grid, conv::smoothing_fraction(f)));
    }

    /// `power` power-smoothed on the columns; gaps stay gaps.
    fn smoothed(&self, sm: &Smoother) -> Vec<f64> {
        let h: Vec<Complex64> = self
            .power
            .iter()
            .map(|p| Complex64::new(p.sqrt(), 0.0))
            .collect();
        let valid: Vec<bool> = self
            .power
            .iter()
            .map(|p| p.is_finite() && *p > 0.0)
            .collect();
        let coherence = vec![0.0; h.len()];
        let out = sm.smooth(
            TfColumns {
                h: &h,
                coherence: &coherence,
                valid: &valid,
            },
            SmoothingMode::Magnitude,
        );
        out.h
            .iter()
            .zip(&out.valid)
            .map(|(z, ok)| if *ok { z.norm_sqr() } else { f64::NAN })
            .collect()
    }
}

/// The multi-resolution bands at `fs` (each framed at the spectrum's display hop for its
/// length).
pub(crate) fn ladder(fs: u32) -> Result<MultiResLadder, String> {
    MultiResLadder::new(f64::from(fs), |n| {
        WireConfig::display_hop(u32::try_from(n).unwrap_or(u32::MAX), fs) as usize
    })
    .map_err(|e| e.to_string())
}

fn multi_grid(grid: Resolution, fs: u32) -> GridDef {
    let g = MultiResLadder::grid(f64::from(fs), grid.ppo());
    GridDef::Log {
        ppo: g.ppo,
        k_min: g.k_min,
        k_max: g.k_max,
    }
}

/// The live (display) grid of a spectrum at `fs`.
pub(crate) fn grid(cfg: &WireConfig, fs: u32) -> GridDef {
    match cfg.resolution {
        SpecResolution::Fixed { fft_len } => GridDef::LogBins {
            fs: Hz(f64::from(fs)),
            n: fft_len,
            ppo: DISPLAY_PPO,
        },
        SpecResolution::Multi { grid } => multi_grid(grid, fs),
    }
}

/// The grid of a spectrum's capture: every FFT bin; a multi-resolution spectrum's columns.
pub(crate) fn capture_grid(cfg: &WireConfig, fs: u32) -> GridDef {
    match cfg.resolution {
        SpecResolution::Fixed { fft_len } => GridDef::Linear {
            fs: Hz(f64::from(fs)),
            n: fft_len,
        },
        SpecResolution::Multi { grid } => multi_grid(grid, fs),
    }
}

impl Spectrum {
    pub(crate) fn new(
        meas: MeasId,
        cfg: WireConfig,
        sample_rate: u32,
        idx: usize,
        cal: InputCal,
        config_rev: Rev,
    ) -> Result<Self, String> {
        let fs = f64::from(sample_rate);
        let window = conv::window(cfg.window);
        let averaging = conv::spec_averaging(cfg.averaging).ok_or("invalid averaging")?;
        let engine = match cfg.resolution {
            SpecResolution::Fixed { fft_len } => {
                let analyzer = SpectrumAnalyzer::new(SpectrumConfig {
                    fs,
                    n: fft_len as usize,
                    hop: WireConfig::display_hop(fft_len, sample_rate) as usize,
                    window,
                    averaging,
                    peak_hold: None,
                })
                .map_err(|e| e.to_string())?;
                let bins = analyzer.bins();
                let smoother = cfg
                    .smoothing
                    .map(|f| LinearSmoother::new(bins, conv::smoothing_fraction(f)));
                Engine::Fixed(Box::new(Fixed {
                    first_bin: BinColumns::new(fs, fft_len, DISPLAY_PPO).first_bin,
                    analyzer,
                    gain: Vec::new(),
                    smoother,
                    power: vec![0.0; bins],
                    smoothed: Vec::new(),
                }))
            }
            SpecResolution::Multi { grid: resolution } => {
                let ladder = ladder(sample_rate)?;
                let grid = MultiResLadder::grid(fs, resolution.ppo());
                let unresolved = ladder
                    .unresolved_ranges(&grid)
                    .into_iter()
                    .map(|(lo, hi)| FreqRange {
                        lo: Hz(lo),
                        hi: Hz(hi),
                    })
                    .collect();
                let analyzer = MultiResAnalyzer::new(
                    ladder,
                    MultiResConfig {
                        window,
                        averaging,
                        grid,
                    },
                )
                .map_err(|e| e.to_string())?;
                let mut m = Multi {
                    power: vec![0.0; analyzer.columns()],
                    analyzer,
                    grid,
                    resolution,
                    smoother: None,
                    unresolved,
                };
                m.set_smoothing(cfg.smoothing);
                Engine::Multi(Box::new(m))
            }
        };
        Ok(Self {
            grid_id: grid(&cfg, sample_rate).id(),
            capture_grid_id: capture_grid(&cfg, sample_rate).id(),
            levels: LevelsMeter::new(vec![idx], vec![cfg.input], sample_rate),
            meas,
            cfg,
            idx,
            engine,
            generation: 0,
            pace: Pace::new(std::time::Duration::ZERO),
            cal: InputCal::none(),
            config_rev,
            applied_at: None,
            buf: Vec::new(),
            end: None,
            wall: 0,
            result: None,
            fresh: false,
        }
        .with_cal(cal))
    }

    fn with_cal(mut self, cal: InputCal) -> Self {
        self.set_cal(cal);
        self
    }

    fn set_cal(&mut self, cal: InputCal) {
        let off = cal.sensitivity.unwrap_or(0.0);
        let gain_at = |f: f64| {
            let c = cal.correction.as_ref().map_or(0.0, |c| c.db(f));
            10f64.powf((off - c) / 10.0)
        };
        match &mut self.engine {
            Engine::Fixed(e) => {
                let df = e.analyzer.config().fs / e.analyzer.config().n as f64;
                let tone = e.analyzer.tone_power_factor();
                e.gain = (0..e.analyzer.bins())
                    .map(|k| tone * gain_at(k as f64 * df))
                    .collect();
            }
            Engine::Multi(e) => e.analyzer.set_gain(gain_at),
        }
        self.cal = cal;
        self.generation += 1;
    }

    fn fs(&self) -> f64 {
        match &self.engine {
            Engine::Fixed(e) => e.analyzer.config().fs,
            Engine::Multi(e) => e.analyzer.ladder().fs,
        }
    }

    fn reset_average(&mut self) {
        match &mut self.engine {
            Engine::Fixed(e) => e.analyzer.reset_average(),
            Engine::Multi(e) => e.analyzer.reset_average(),
        }
    }
}

impl Spectrum {
    /// The stamp of the newest block, `end` exclusive.
    fn stamp(&self, end: u64) -> StampArgs {
        self.stamp_at(end, self.wall)
    }

    /// The stamp of a frame carrying the newest analysed spectrum: at its own end when it
    /// has not gone out yet, else (a setting changed, or a refresh) at the newest block.
    fn result_stamp(&self, end: u64) -> StampArgs {
        match self.result.filter(|_| self.fresh) {
            Some((r_end, r_wall)) => self.stamp_at(r_end, r_wall),
            None => self.stamp(end),
        }
    }

    fn stamp_at(&self, end: u64, wall: u64) -> StampArgs {
        StampArgs {
            audio_sample: end.saturating_sub(1),
            config_rev: self.config_rev,
            applied_at: self.applied_at.unwrap_or(0),
            wall_ns: wall,
            grid_id: Some(self.grid_id),
            protection: if self.levels.any_clip_held() {
                ProtectionFlags::CLIP
            } else {
                ProtectionFlags::NONE
            },
        }
    }

    /// Metadata of a frame whose audio ends at `end` (exclusive).
    fn meta(&self, end: u64) -> SpecMeta {
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
            bands: self.bands(end),
        }
    }

    /// The multi-resolution bands as of audio ending at `end`; `None` for one FFT.
    fn bands(&self, end: u64) -> Option<Box<SpecBands>> {
        let Engine::Multi(e) = &self.engine else {
            return None;
        };
        let fs = self.fs();
        let now = self.end.unwrap_or(0);
        let bands = e
            .analyzer
            .ladder()
            .bands
            .iter()
            .enumerate()
            .map(|(b, band)| SpecBand {
                window: Seconds(band.window_s(fs)),
                hop: Seconds(band.hop_s(fs)),
                sole: Hz(band.anchor_hz),
                age: e.analyzer.samples_since_frame(b).map(|since| {
                    let band_end = now.saturating_sub(since);
                    Seconds(end.saturating_sub(band_end) as f64 / fs)
                }),
            })
            .collect();
        Some(Box::new(SpecBands {
            bands,
            unresolved: Unresolved {
                resolution: e.resolution,
                ranges: e.unresolved.clone(),
            },
        }))
    }

    /// Fills the engine's `power` with the averaged tone power per bin (per column,
    /// multi-resolution); `false` before the first spectrum. A bin without power is a gap
    /// (−∞ dB has no place on a dB axis), as it is in a captured trace; smoothing never
    /// crosses a gap, so a capture re-smoothed at this setting reads the same as the live
    /// frame.
    fn fill_power(&mut self) -> bool {
        match &mut self.engine {
            Engine::Fixed(e) => {
                let Some(ps) = e.analyzer.average() else {
                    return false;
                };
                for ((p, f), g) in e.power.iter_mut().zip(ps.folded()).zip(&e.gain) {
                    let x = f * g;
                    *p = if x > 0.0 { x } else { f64::NAN };
                }
                true
            }
            Engine::Multi(e) => {
                if !e.analyzer.column_power(&mut e.power) {
                    return false;
                }
                for p in &mut e.power {
                    // No power (or none yet) is no level, as for a fixed spectrum's bin.
                    if (*p).partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
                        *p = f64::NAN;
                    }
                }
                true
            }
        }
    }

    /// The live frame's levels from a filled `power`: display columns (smoothed when on).
    fn live_levels(&mut self) -> Vec<f32> {
        match &mut self.engine {
            Engine::Fixed(e) => {
                let bins = e.power.len();
                let shown = match e.smoother.as_mut().filter(|s| s.bins() == bins) {
                    Some(sm) => {
                        e.smoothed.resize(bins, 0.0);
                        sm.smooth_into(&e.power, &mut e.smoothed);
                        &e.smoothed
                    }
                    None => &e.power,
                };
                column_max(shown, &e.first_bin).map(db).collect()
            }
            Engine::Multi(e) => match &e.smoother {
                Some(sm) => e.smoothed(sm).into_iter().map(db).collect(),
                None => e.power.iter().copied().map(db).collect(),
            },
        }
    }

    /// The capture's levels from a filled `power`: every bin (every column), unsmoothed.
    fn capture_levels(&self) -> Vec<f32> {
        let power = match &self.engine {
            Engine::Fixed(e) => &e.power,
            Engine::Multi(e) => &e.power,
        };
        power.iter().map(|p| db(*p)).collect()
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
    fn result_generation(&self) -> Option<u64> {
        Some(self.generation)
    }

    fn push(&mut self, b: &Block) {
        self.applied_at.get_or_insert(b.start_sample);
        if self.end.is_some_and(|e| e != b.start_sample)
            || (self.end.is_some() && b.flags.breaks_continuity())
        {
            self.reset_average();
        }
        channel_f64(b, self.idx, &mut self.buf);
        let since = match &mut self.engine {
            Engine::Fixed(e) => (e.analyzer.push(&self.buf) > 0)
                .then(|| e.analyzer.samples_since_frame())
                .flatten(),
            Engine::Multi(e) => (e.analyzer.push(&self.buf) != 0)
                .then(|| e.analyzer.samples_since_newest())
                .flatten(),
        };
        if let Some(since) = since {
            self.generation += 1;
            let since_ns = since as f64 / self.fs() * 1e9;
            self.result = Some((
                b.end_sample() - since,
                b.wall_ns.saturating_sub(since_ns as u64),
            ));
            self.fresh = true;
        }
        self.levels.push(b);
        self.end = Some(b.end_sample());
        self.wall = b.wall_ns;
    }

    fn command(&mut self, c: JobCmd) {
        match c {
            JobCmd::Reset => self.reset_average(),
            JobCmd::Cal(cal) => self.set_cal(*cal),
            JobCmd::Smoothing {
                change: SmoothingChange::Spectrum(smoothing),
                rev,
            } => {
                self.cfg.smoothing = smoothing;
                match &mut self.engine {
                    Engine::Fixed(e) => {
                        let bins = e.analyzer.bins();
                        e.smoother = smoothing
                            .map(|f| LinearSmoother::new(bins, conv::smoothing_fraction(f)));
                    }
                    Engine::Multi(e) => e.set_smoothing(smoothing),
                }
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
            let at = self.result_stamp(end);
            let live = SpecFrame {
                meas: self.meas,
                meta: self.meta(at.audio_sample + 1),
                level: self.live_levels(),
            };
            if e.send(
                StampArgs {
                    protection: stamp.protection,
                    ..at
                },
                FrameData::Spec(live),
            ) {
                self.fresh = false;
            } else {
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
        let stamp = StampArgs {
            grid_id: Some(self.capture_grid_id),
            ..self
                .result
                .map_or_else(|| self.stamp(end), |(e, w)| self.stamp_at(e, w))
        };
        let capture = SpecFrame {
            meas: self.meas,
            meta: self.meta(stamp.audio_sample + 1),
            level: self.capture_levels(),
        };
        Some((stamp, FrameData::Spec(capture)))
    }
}
