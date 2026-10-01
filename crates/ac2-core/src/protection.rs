//! Data protection: clip rejection, missing-reference pause, weak-reference hold.
//!
//! A transfer-function job feeds each block's levels through [`Guard::process`] before
//! accumulating spectra. The decision depends only on physics:
//!
//! - **Clip**: a sample at or above the clip threshold on either channel means the
//!   converter's transfer is no longer linear for that block; its spectra are wrong at every
//!   frequency, so the block is thrown away.
//! - **No reference**: below the reference floor `Gxx` is noise, and H1 = Gxy/Gxx divides
//!   by it. The job pauses: nothing is accumulated.
//! - **Weak reference**: a reference far below its own running level (a pause in programme
//!   material, a stimulus dropout) adds blocks whose cross-spectrum is dominated by noise
//!   and drags coherence down. The average is held — not reset — until the level returns.
//!
//! Levels: RMS in dBFS with 0 dBFS = a full-scale sine (RMS 1/√2, decision 4a); peaks in
//! dBFS of the absolute sample value (full scale = 1.0).
//!
//! Banners report persistent conditions to the operator with hysteresis in both level and
//! time, so a signal sitting at a threshold does not flicker a banner on and off.
//! **NO SIGNAL** (measurement channel below its floor while the reference is present) is a
//! banner only: a silent measurement channel is still a valid measurement of a muted or
//! disconnected system, and its low coherence already says so on the trace.

/// Thresholds and timing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProtectionConfig {
    /// A sample peak at or above this on any channel rejects the block, dBFS.
    pub clip_threshold_dbfs: f64,
    /// Reference RMS below this pauses the job, dBFS.
    pub reference_floor_dbfs: f64,
    /// Measurement RMS below this raises NO SIGNAL, dBFS.
    pub signal_floor_dbfs: f64,
    /// Reference more than this far below its running level holds the average, dB (> 0).
    pub weak_reference_db: f64,
    /// Time constant of the running reference level, seconds.
    pub running_level_tau_s: f64,
    /// Level hysteresis for clearing NO REFERENCE / NO SIGNAL: the level must exceed the
    /// floor by this much, dB.
    pub level_hysteresis_db: f64,
    /// A level condition must persist this long before its banner is raised, seconds.
    pub raise_after_s: f64,
    /// A level banner clears after its condition has been absent this long, seconds.
    pub clear_after_s: f64,
    /// CLIP stays raised this long after the last clipped block, seconds.
    pub clip_hold_s: f64,
}

impl Default for ProtectionConfig {
    fn default() -> Self {
        Self {
            clip_threshold_dbfs: -0.1,
            reference_floor_dbfs: -80.0,
            signal_floor_dbfs: -90.0,
            weak_reference_db: 20.0,
            running_level_tau_s: 2.0,
            level_hysteresis_db: 3.0,
            raise_after_s: 0.25,
            clear_after_s: 0.5,
            clip_hold_s: 1.0,
        }
    }
}

/// Levels of one block of a reference/measurement pair, linear full-scale units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockLevels {
    /// Largest |sample| on the reference.
    pub reference_peak: f64,
    /// Largest |sample| on the measurement.
    pub measurement_peak: f64,
    /// RMS of the reference.
    pub reference_rms: f64,
    /// RMS of the measurement.
    pub measurement_rms: f64,
    /// Block length, seconds.
    pub duration_s: f64,
}

impl BlockLevels {
    /// Levels of equal-length sample blocks at `sample_rate`.
    pub fn from_samples(reference: &[f32], measurement: &[f32], sample_rate: f64) -> Self {
        let stats = |x: &[f32]| {
            let (peak, sq) = x.iter().fold((0.0f64, 0.0f64), |(p, s), v| {
                let v = f64::from(*v);
                (p.max(v.abs()), s + v * v)
            });
            let rms = if x.is_empty() {
                0.0
            } else {
                (sq / x.len() as f64).sqrt()
            };
            (peak, rms)
        };
        let (reference_peak, reference_rms) = stats(reference);
        let (measurement_peak, measurement_rms) = stats(measurement);
        Self {
            reference_peak,
            measurement_peak,
            reference_rms,
            measurement_rms,
            duration_s: reference.len().max(measurement.len()) as f64 / sample_rate,
        }
    }
}

/// Peak level in dBFS (full scale = 1.0).
pub fn peak_dbfs(peak: f64) -> f64 {
    20.0 * peak.log10()
}

/// RMS level in dBFS, 0 dBFS = full-scale sine.
pub fn rms_dbfs(rms: f64) -> f64 {
    20.0 * (rms * std::f64::consts::SQRT_2).log10()
}

/// What to do with a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockDecision {
    /// Accumulate.
    Accept,
    /// Discard: a channel clipped.
    RejectClip,
    /// Do not accumulate: no reference.
    PauseNoReference,
    /// Do not accumulate, keep the average: reference far below its running level.
    HoldWeakReference,
}

/// Blocks seen per decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProtectionCounters {
    /// Accepted blocks.
    pub accepted: u64,
    /// Blocks rejected for clipping.
    pub clipped: u64,
    /// Blocks paused for no reference.
    pub no_reference: u64,
    /// Blocks held for a weak reference.
    pub weak_reference: u64,
}

/// Banners currently shown. Several may be up at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Banners {
    /// Reference below its floor.
    pub no_reference: bool,
    /// Measurement below its floor while the reference is present.
    pub no_signal: bool,
    /// A channel clipped recently.
    pub clip: bool,
}

/// Time hysteresis: raise after the condition has held for `raise_after`, clear after it
/// has been absent for `clear_after`. "Absent" is decided by a separate, stricter test so
/// level hysteresis and time hysteresis combine.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Latch {
    up: bool,
    present_for: f64,
    absent_for: f64,
}

impl Latch {
    fn step(&mut self, present: bool, clearly_absent: bool, dt: f64, raise: f64, clear: f64) {
        if present {
            self.present_for += dt;
            self.absent_for = 0.0;
        } else if clearly_absent {
            self.absent_for += dt;
            self.present_for = 0.0;
        } else {
            // Inside the hysteresis band: neither timer runs on, neither resets the state.
            self.present_for = 0.0;
            self.absent_for = 0.0;
        }
        if !self.up && present && self.present_for >= raise {
            self.up = true;
        } else if self.up && clearly_absent && self.absent_for >= clear {
            self.up = false;
        }
    }
}

/// Per-job protection state.
#[derive(Debug, Clone, PartialEq)]
pub struct Guard {
    config: ProtectionConfig,
    /// Running mean-square reference level (linear), once seeded.
    running_ms: Option<f64>,
    counters: ProtectionCounters,
    no_reference: Latch,
    no_signal: Latch,
    since_clip: Option<f64>,
}

impl Guard {
    /// Fresh state.
    pub fn new(config: ProtectionConfig) -> Self {
        Self {
            config,
            running_ms: None,
            counters: ProtectionCounters::default(),
            no_reference: Latch::default(),
            no_signal: Latch::default(),
            since_clip: None,
        }
    }

    /// Configuration.
    pub fn config(&self) -> &ProtectionConfig {
        &self.config
    }

    /// Decide on one block and update counters and banners.
    pub fn process(&mut self, b: &BlockLevels) -> BlockDecision {
        let c = self.config;
        let dt = b.duration_s.max(0.0);
        let clipped =
            peak_dbfs(b.reference_peak).max(peak_dbfs(b.measurement_peak)) >= c.clip_threshold_dbfs;
        let ref_db = rms_dbfs(b.reference_rms);
        let meas_db = rms_dbfs(b.measurement_rms);
        // A NaN level (corrupt samples) counts as missing, never as present.
        let ref_missing = ref_db.is_nan() || ref_db < c.reference_floor_dbfs;
        let ref_back = ref_db >= c.reference_floor_dbfs + c.level_hysteresis_db;

        self.no_reference
            .step(ref_missing, ref_back, dt, c.raise_after_s, c.clear_after_s);
        // NO SIGNAL only makes a statement while the reference is there.
        let sig_missing = !ref_missing && (meas_db.is_nan() || meas_db < c.signal_floor_dbfs);
        let sig_back = ref_missing || meas_db >= c.signal_floor_dbfs + c.level_hysteresis_db;
        self.no_signal
            .step(sig_missing, sig_back, dt, c.raise_after_s, c.clear_after_s);
        self.since_clip = if clipped {
            Some(0.0)
        } else {
            self.since_clip.map(|t| t + dt)
        };

        let decision = if clipped {
            BlockDecision::RejectClip
        } else if ref_missing {
            BlockDecision::PauseNoReference
        } else {
            let ms = b.reference_rms * b.reference_rms;
            let weak = self
                .running_ms
                .is_some_and(|run| 10.0 * (ms / run).log10() < -c.weak_reference_db);
            // The running level follows held blocks too, slowly, so a lasting change of
            // stimulus level becomes the new normal instead of holding forever.
            let alpha = if c.running_level_tau_s > 0.0 {
                1.0 - (-dt / c.running_level_tau_s).exp()
            } else {
                1.0
            };
            self.running_ms = Some(match self.running_ms {
                Some(run) => run + alpha * (ms - run),
                None => ms,
            });
            if weak {
                BlockDecision::HoldWeakReference
            } else {
                BlockDecision::Accept
            }
        };
        match decision {
            BlockDecision::Accept => self.counters.accepted += 1,
            BlockDecision::RejectClip => self.counters.clipped += 1,
            BlockDecision::PauseNoReference => self.counters.no_reference += 1,
            BlockDecision::HoldWeakReference => self.counters.weak_reference += 1,
        }
        decision
    }

    /// Counters since construction or [`Guard::reset_counters`].
    pub fn counters(&self) -> ProtectionCounters {
        self.counters
    }

    /// Zero the counters (e.g. on an average reset); banners and running level are kept.
    pub fn reset_counters(&mut self) {
        self.counters = ProtectionCounters::default();
    }

    /// Banners to show now.
    pub fn banners(&self) -> Banners {
        Banners {
            no_reference: self.no_reference.up,
            no_signal: self.no_signal.up,
            clip: self.since_clip.is_some_and(|t| t < self.config.clip_hold_s),
        }
    }

    /// Running reference level in dBFS RMS, once any reference has been seen.
    pub fn running_reference_dbfs(&self) -> Option<f64> {
        self.running_ms.map(|ms| rms_dbfs(ms.sqrt()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact in binary, so accumulated block times hit the thresholds exactly.
    const DT: f64 = 0.0625;

    /// Levels for a block with RMS levels in dBFS (full-scale-sine convention) and
    /// sine-like peaks.
    fn block(ref_db: f64, meas_db: f64) -> BlockLevels {
        let rms = |db: f64| 10f64.powf(db / 20.0) / std::f64::consts::SQRT_2;
        BlockLevels {
            reference_peak: rms(ref_db) * std::f64::consts::SQRT_2,
            measurement_peak: rms(meas_db) * std::f64::consts::SQRT_2,
            reference_rms: rms(ref_db),
            measurement_rms: rms(meas_db),
            duration_s: DT,
        }
    }

    #[test]
    fn levels_from_samples() {
        let n = 4800;
        let sine: Vec<f32> = (0..n)
            .map(|i| (std::f64::consts::TAU * 1000.0 * i as f64 / 48000.0).sin() as f32)
            .collect();
        let zero = vec![0.0f32; n];
        let b = BlockLevels::from_samples(&sine, &zero, 48000.0);
        assert!(
            rms_dbfs(b.reference_rms).abs() < 1e-3,
            "full-scale sine is 0 dBFS"
        );
        assert!((b.duration_s - 0.1).abs() < 1e-12);
        assert_eq!(b.measurement_peak, 0.0);
        assert!(rms_dbfs(b.measurement_rms) == f64::NEG_INFINITY);
    }

    #[test]
    fn decisions() {
        let mut g = Guard::new(ProtectionConfig::default());
        assert_eq!(g.process(&block(-20.0, -25.0)), BlockDecision::Accept);
        let mut clip = block(-20.0, -25.0);
        clip.measurement_peak = 1.0;
        assert_eq!(g.process(&clip), BlockDecision::RejectClip);
        assert_eq!(
            g.process(&block(-100.0, -25.0)),
            BlockDecision::PauseNoReference
        );
        assert_eq!(
            g.process(&block(f64::NEG_INFINITY, -25.0)),
            BlockDecision::PauseNoReference
        );
        assert_eq!(
            g.process(&block(-50.0, -55.0)),
            BlockDecision::HoldWeakReference
        );
        assert_eq!(g.process(&block(-21.0, -25.0)), BlockDecision::Accept);
        assert_eq!(
            g.counters(),
            ProtectionCounters {
                accepted: 2,
                clipped: 1,
                no_reference: 2,
                weak_reference: 1,
            }
        );
    }

    #[test]
    fn lasting_level_drop_becomes_normal() {
        let mut g = Guard::new(ProtectionConfig::default());
        for _ in 0..100 {
            g.process(&block(-10.0, -10.0));
        }
        assert_eq!(
            g.process(&block(-40.0, -40.0)),
            BlockDecision::HoldWeakReference
        );
        let mut accepted_after = None;
        for k in 0..400 {
            if g.process(&block(-40.0, -40.0)) == BlockDecision::Accept {
                accepted_after = Some(k);
                break;
            }
        }
        let k = accepted_after.expect("running level adapts");
        // ln(10^(30/10) / 10^(20/10)) = ln 10 time constants ≈ 4.6 s at τ = 2 s.
        let t = k as f64 * DT;
        assert!((3.5..6.0).contains(&t), "adapted after {t} s");
    }

    #[test]
    fn no_reference_banner_hysteresis() {
        let cfg = ProtectionConfig::default();
        let mut g = Guard::new(cfg);
        // Short dropout below raise time: no banner.
        for _ in 0..3 {
            g.process(&block(-100.0, -100.0));
        }
        assert!(!g.banners().no_reference);
        g.process(&block(-20.0, -20.0));
        // Sustained: raised after 0.25 s (4 blocks).
        for k in 0..4 {
            assert!(!g.banners().no_reference, "raised early at block {k}");
            g.process(&block(-100.0, -100.0));
        }
        assert!(g.banners().no_reference);
        // NO SIGNAL is not stated while the reference is missing.
        assert!(!g.banners().no_signal);
        // A level hovering just above the floor (inside the 3 dB band) never clears it.
        for k in 0..100 {
            let db = if k % 2 == 0 { -79.0 } else { -81.0 };
            g.process(&block(db, -20.0));
            assert!(g.banners().no_reference, "flickered at block {k}");
        }
        // Clearly back: clears after 0.5 s (8 blocks), not before.
        for _ in 0..7 {
            g.process(&block(-30.0, -30.0));
            assert!(g.banners().no_reference);
        }
        g.process(&block(-30.0, -30.0));
        assert!(!g.banners().no_reference);
    }

    #[test]
    fn no_signal_banner() {
        let mut g = Guard::new(ProtectionConfig::default());
        for _ in 0..10 {
            assert_eq!(g.process(&block(-20.0, -120.0)), BlockDecision::Accept);
        }
        assert!(g.banners().no_signal && !g.banners().no_reference);
        for _ in 0..10 {
            g.process(&block(-20.0, -20.0));
        }
        assert!(!g.banners().no_signal);
    }

    #[test]
    fn clip_banner_holds() {
        let mut g = Guard::new(ProtectionConfig::default());
        let mut clip = block(-20.0, -20.0);
        clip.reference_peak = 10f64.powf(-0.05 / 20.0);
        assert_eq!(g.process(&clip), BlockDecision::RejectClip);
        assert!(g.banners().clip);
        // Held for 1 s (16 blocks) after the last clip, then cleared.
        for _ in 0..15 {
            g.process(&block(-20.0, -20.0));
            assert!(g.banners().clip);
        }
        g.process(&block(-20.0, -20.0));
        assert!(!g.banners().clip);
        // Just under the threshold is not a clip.
        let mut near = block(-20.0, -20.0);
        near.reference_peak = 10f64.powf(-0.2 / 20.0);
        assert_eq!(g.process(&near), BlockDecision::Accept);
    }
}
