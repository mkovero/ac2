//! Stimulus output: lease enforcement inside the output path and the level ceiling.
//!
//! The lease deadline is an atomic the control thread moves forward on every refresh. The
//! signal source checks it at the start of every fill, i.e. on the audio callback, so an
//! expired lease silences the output even if the control thread is stuck (Q6): the source
//! mutes its generator, whose gain ramps to zero over 20 ms (decision 6b), stays silent for
//! good, and raises the gate's `tripped` flag so the control thread disarms and says so:
//! the state never shows a firing generator whose output path has muted itself. A source
//! that has not played yet only waits for the gate: it outputs zeros while the gate is
//! closed and latches nothing, so no ordering of gate and source can silence a stimulus
//! before it starts. Reading a monotonic clock there is a vDSO / QPC / mach read, not a
//! blocking system call; no allocation or lock is involved.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use ac2_audio::{LevelError, MaxLevel, SignalSource};
use ac2_core::generator::{FILTERED_NOISE_CREST, Generator, LevelControl, dbfs_to_rms};

/// Shared lease deadline.
#[derive(Debug)]
pub(crate) struct LeaseGate {
    origin: Instant,
    /// Deadline in ns since `origin`; 0 = closed.
    deadline_ns: AtomicU64,
    /// A playing source found the deadline passed and muted itself.
    tripped: AtomicBool,
}

impl LeaseGate {
    pub(crate) fn new() -> Self {
        Self {
            origin: Instant::now(),
            deadline_ns: AtomicU64::new(0),
            tripped: AtomicBool::new(false),
        }
    }

    fn now_ns(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// Output may continue until `at`.
    pub(crate) fn open_until(&self, at: Instant) {
        let ns = u64::try_from(at.saturating_duration_since(self.origin).as_nanos())
            .unwrap_or(u64::MAX)
            .max(1);
        self.deadline_ns.store(ns, Ordering::Release);
    }

    /// Output must fade now.
    pub(crate) fn close(&self) {
        self.deadline_ns.store(0, Ordering::Release);
    }

    fn expired(&self) -> bool {
        self.now_ns() >= self.deadline_ns.load(Ordering::Acquire)
    }

    /// Whether a playing source muted itself on an expired deadline since the last call.
    pub(crate) fn take_tripped(&self) -> bool {
        self.tripped.swap(false, Ordering::AcqRel)
    }
}

/// Where a [`LeasedSource`] is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Never played: zeros while the gate is closed, nothing latched.
    Waiting,
    /// Played under an open gate.
    Playing,
    /// The deadline passed while playing: faded out for good.
    Expired,
}

/// A generator that fades itself out for good once the lease deadline passes while it
/// plays.
pub(crate) struct LeasedSource {
    generator: Generator,
    level: LevelControl,
    gate: Arc<LeaseGate>,
    phase: Phase,
}

impl LeasedSource {
    pub(crate) fn new(generator: Generator, gate: Arc<LeaseGate>) -> Self {
        Self {
            level: generator.level_control(),
            generator,
            gate,
            phase: Phase::Waiting,
        }
    }
}

impl SignalSource for LeasedSource {
    fn fill(&mut self, out: &mut [f32]) {
        let expired = self.gate.expired();
        match (self.phase, expired) {
            (Phase::Waiting, true) => {
                out.fill(0.0);
                return;
            }
            (Phase::Waiting, false) => self.phase = Phase::Playing,
            (Phase::Playing, true) => {
                self.phase = Phase::Expired;
                // An atomic store: the generator ramps to zero over its 20 ms ramp.
                self.level.fade_out();
                // A closed gate is the control side's own stop; only a deadline that
                // passed on its own is news to it.
                if self.gate.deadline_ns.load(Ordering::Acquire) != 0 {
                    self.gate.tripped.store(true, Ordering::Release);
                }
            }
            (Phase::Playing, false) | (Phase::Expired, _) => {}
        }
        self.generator.fill(out);
    }
}

/// The sample-peak limit the output path enforces for an RMS ceiling: the largest crest
/// factor any generator signal is allowed (filtered noise, 6) times the ceiling RMS, capped
/// at full scale. The generator refuses levels whose own crest would exceed it, so the limit
/// only ever acts on a computation error upstream.
pub(crate) fn peak_limit(ceiling_dbfs: f64) -> Result<MaxLevel, LevelError> {
    if !ceiling_dbfs.is_finite() {
        return Err(LevelError::NotFinite);
    }
    let peak = (dbfs_to_rms(ceiling_dbfs) * FILTERED_NOISE_CREST).min(1.0);
    MaxLevel::from_peak_db((20.0 * peak.max(1e-12).log10()).min(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_core::generator::{BandLimit, GeneratorConfig, Signal};
    use std::time::Duration;

    fn source(gate: Arc<LeaseGate>) -> LeasedSource {
        let g = Generator::new(&GeneratorConfig {
            signal: Signal::Sine { freq_hz: 1000.0 },
            sample_rate: 48_000.0,
            seed: 1,
            band: BandLimit::NONE,
            level_dbfs: -20.0,
            ceiling_dbfs: -10.0,
        })
        .expect("generator");
        LeasedSource::new(g, gate)
    }

    #[test]
    fn expired_lease_fades_within_20ms_and_stays_silent() {
        let gate = Arc::new(LeaseGate::new());
        gate.open_until(Instant::now() + Duration::from_secs(60));
        let mut s = source(Arc::clone(&gate));
        let mut buf = vec![0.0f32; 4800];
        s.fill(&mut buf);
        assert!(
            buf[2000..].iter().any(|v| v.abs() > 0.05),
            "plays while leased"
        );
        gate.close();
        let mut fade = vec![0.0f32; 960];
        s.fill(&mut fade);
        let mut after = vec![0.0f32; 4800];
        s.fill(&mut after);
        assert!(after.iter().all(|v| *v == 0.0), "silent after the fade");
        assert!(!gate.take_tripped(), "a deliberate close is not an expiry");
        // Re-opening the gate does not revive a source that expired.
        gate.open_until(Instant::now() + Duration::from_secs(60));
        s.fill(&mut after);
        assert!(after.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn a_source_waits_for_the_gate_without_latching() {
        let gate = Arc::new(LeaseGate::new());
        let mut s = source(Arc::clone(&gate));
        let mut buf = vec![1.0f32; 4800];
        // Installed before the gate opened: zeros, and nothing is latched or reported.
        s.fill(&mut buf);
        assert!(buf.iter().all(|v| *v == 0.0));
        assert!(!gate.take_tripped());
        gate.open_until(Instant::now() + Duration::from_secs(60));
        s.fill(&mut buf);
        assert!(
            buf[2000..].iter().any(|v| v.abs() > 0.05),
            "plays once open"
        );
    }

    #[test]
    fn expiry_while_playing_is_reported_once() {
        let gate = Arc::new(LeaseGate::new());
        gate.open_until(Instant::now() + Duration::from_secs(60));
        let mut s = source(Arc::clone(&gate));
        let mut buf = vec![0.0f32; 480];
        s.fill(&mut buf);
        assert!(!gate.take_tripped());
        // The deadline passes on its own.
        gate.open_until(Instant::now());
        s.fill(&mut buf);
        assert!(gate.take_tripped(), "the control thread learns of the mute");
        assert!(!gate.take_tripped());
        s.fill(&mut buf);
        assert!(!gate.take_tripped(), "reported once");
    }

    #[test]
    fn peak_limit_follows_ceiling() {
        let m = peak_limit(-20.0).expect("finite");
        // -20 dBFS RMS = 0.0707 FS RMS; × 6 = 0.424 FS peak ≈ -7.4 dB.
        assert!(
            (m.peak_db() - (20.0 * (0.1 * std::f64::consts::FRAC_1_SQRT_2 * 6.0f64).log10())).abs()
                < 1e-9
        );
        assert_eq!(peak_limit(0.0).expect("finite").peak_db(), 0.0);
        assert!(peak_limit(f64::NAN).is_err());
    }
}
