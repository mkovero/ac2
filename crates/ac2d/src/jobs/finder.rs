//! Delay finder and tracking for a transfer job (design Q1), on the raw, unaligned
//! reference / measurement pair: the finder's result is the absolute delay, never relative
//! to the delay the MTW currently applies.

use std::collections::VecDeque;

use ac2_core::delay::{
    Agreement, Band, Block as DBlock, Confidence, DelayStream, FinderConfig, FinderResult,
    FinderScratch, NoEstimateReason, Outcome, Tracker, find_auto_with, find_with,
};

use crate::conv::FindBand;

/// Longest observation `delay.find` accepts (decision D2: the sub band's 8 s choice).
pub(crate) const MAX_OBSERVATION_S: f64 = 8.0;
/// Seconds of raw audio kept for `delay.find`: the longest observation plus the ±1 s search
/// span on both sides, with margin.
const HISTORY_S: f64 = MAX_OBSERVATION_S + 3.0;
/// Default observation of an auto-band run: the longest band default (sub), so that the sub
/// band, tried last, has its full observation when the higher bands refuse.
const AUTO_OBSERVATION_S: f64 = 4.0;

pub(crate) struct Finder {
    fs: f64,
    cap: usize,
    reference: VecDeque<f32>,
    measurement: VecDeque<f32>,
    start: u64,
    scratch: FinderScratch,
    /// Band tracking runs in: the band of the last finding that was not refused.
    track_band: Band,
    tracking: Option<(DelayStream, Tracker)>,
    /// An ambiguous finding awaits the operator's pick (decision 1c): tracking observes
    /// nothing until it is resolved, so it cannot move the delay to one of the candidates
    /// on its own.
    paused: bool,
}

impl Finder {
    pub(crate) fn new(fs: f64) -> Self {
        Self {
            fs,
            cap: (HISTORY_S * fs) as usize,
            reference: VecDeque::new(),
            measurement: VecDeque::new(),
            start: 0,
            scratch: FinderScratch::new(),
            track_band: Band::FullRange,
            tracking: None,
            paused: false,
        }
    }

    /// A stream gap: nothing before it can be spliced onto what follows.
    pub(crate) fn restart(&mut self) {
        self.reference.clear();
        self.measurement.clear();
        if let Some((s, t)) = &mut self.tracking {
            s.reset();
            t.reset();
        }
    }

    /// Appends a block; returns a new tracked delay when tracking moves it.
    pub(crate) fn push(&mut self, start: u64, r: &[f32], m: &[f32]) -> Option<i64> {
        if self.reference.is_empty() || start != self.start + self.reference.len() as u64 {
            self.reference.clear();
            self.measurement.clear();
            self.start = start;
        }
        self.reference.extend(r);
        self.measurement.extend(m);
        let excess = self.reference.len().saturating_sub(self.cap);
        if excess > 0 {
            self.reference.drain(..excess);
            self.measurement.drain(..excess);
            self.start += excess as u64;
        }
        if self.paused {
            return None;
        }
        let (stream, tracker) = self.tracking.as_mut()?;
        stream.push_ref(DBlock { start, samples: r });
        stream.push_meas(DBlock { start, samples: m });
        let mut moved = None;
        loop {
            match stream.poll() {
                Ok(Some(res)) => {
                    if let Some(d) = tracker.observe(&res) {
                        moved = Some(d);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("delay tracking: {e}");
                    break;
                }
            }
        }
        moved
    }

    /// Turns tracking on (from the held delay) or off.
    pub(crate) fn track(&mut self, enabled: bool, held: i64) {
        self.tracking = if enabled {
            match DelayStream::new(FinderConfig::new(self.fs, self.track_band)) {
                Ok(s) => {
                    let mut t = Tracker::new(Agreement::for_band(self.track_band, self.fs));
                    t.set_held(Some(held));
                    Some((s, t))
                }
                Err(e) => {
                    tracing::warn!("delay tracking not started: {e}");
                    None
                }
            }
        } else {
            None
        };
    }

    /// Pauses tracking while an ambiguous finding awaits a pick, or resumes it. A resumed
    /// stream starts over: the audio skipped while paused cannot be spliced onto what
    /// follows, and a result pending from before the pause no longer compares.
    pub(crate) fn set_paused(&mut self, paused: bool) {
        if self.paused
            && !paused
            && let Some((s, t)) = &mut self.tracking
        {
            s.reset();
            t.reset();
        }
        self.paused = paused;
    }

    /// Tracking is paused (an ambiguous finding awaits a pick).
    #[cfg(test)]
    pub(crate) fn paused(&self) -> bool {
        self.paused
    }

    /// The operator set the delay.
    pub(crate) fn set_held(&mut self, held: i64) {
        if let Some((_, t)) = &mut self.tracking {
            t.set_held(Some(held));
        }
    }

    /// Runs the finder on the newest observation that has reference coverage for every lag
    /// of the ±1 s search. `observation` (seconds) asks for exactly that block; `None` takes
    /// what has been captured, up to the band's default. Too little audio is a refusal
    /// (`ObservationTooShort`), not an error; `Err` is a configuration the finder rejects.
    /// A finding that is not refused moves tracking to its band. A new finding resumes
    /// tracking unless it is ambiguous, which pauses it until the operator picks.
    pub(crate) fn find(
        &mut self,
        band: FindBand,
        observation: Option<f64>,
        held: i64,
    ) -> Result<FinderResult, String> {
        let (core_band, default_s) = match band {
            FindBand::Auto => (Band::FullRange, AUTO_OBSERVATION_S),
            FindBand::Band(b) => (b, b.class().default_observation_s()),
        };
        let mut cfg = FinderConfig::new(self.fs, core_band);
        cfg.observation_s = observation;
        cfg.validate().map_err(|e| e.to_string())?;
        let lead = cfg.search.max.max(0) as usize;
        let tail = cfg.search.min.min(0).unsigned_abs() as usize;
        let total = self.measurement.len();
        let room = total.saturating_sub(lead + tail);
        let (want, need) = match observation {
            Some(s) => {
                let n = (s * self.fs).round() as usize;
                (n, n.max(cfg.min_observation_len()))
            }
            None => (
                (default_s * self.fs).round() as usize,
                cfg.min_observation_len(),
            ),
        };
        if room < need {
            let end = self.start + total as u64;
            self.set_paused(false);
            return Ok(FinderResult {
                outcome: Outcome::NoEstimate {
                    reasons: vec![NoEstimateReason::ObservationTooShort],
                },
                candidates: Vec::new(),
                confidence: Confidence {
                    psr_db: f64::NAN,
                    psr_acq_db: f64::NAN,
                    band_snr_db: f64::NAN,
                    excited_fraction: f64::NAN,
                    pulse_width: f64::NAN,
                    nominal_width: f64::NAN,
                    period: None,
                    refinement_window: None,
                },
                band: core_band,
                meas_window: end - room as u64..end,
            });
        }
        let lm = room.min(want);
        let m0 = total - tail - lm;
        let r0 = m0 - lead;
        let r: Vec<f32> = self.reference.range(r0..).copied().collect();
        let m: Vec<f32> = self.measurement.range(m0..m0 + lm).copied().collect();
        let rb = DBlock {
            start: self.start + r0 as u64,
            samples: &r,
        };
        let mb = DBlock {
            start: self.start + m0 as u64,
            samples: &m,
        };
        let res = match band {
            FindBand::Auto => find_auto_with(&mut self.scratch, rb, mb, &cfg),
            FindBand::Band(_) => find_with(&mut self.scratch, rb, mb, &cfg),
        }
        .map_err(|e| e.to_string())?;
        if !matches!(res.outcome, Outcome::NoEstimate { .. }) && res.band != self.track_band {
            self.track_band = res.band;
            if self.tracking.is_some() {
                self.track(true, held);
            }
        }
        self.set_paused(matches!(res.outcome, Outcome::Ambiguous { .. }));
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x as f64 / u64::MAX as f64 * 2.0 - 1.0) as f32 * 0.5
            })
            .collect()
    }

    #[test]
    fn too_little_audio_is_a_typed_refusal() {
        let mut f = Finder::new(48_000.0);
        let r = noise(48_000, 1);
        f.push(0, &r, &r);
        let res = f.find(FindBand::Auto, None, 0).expect("valid config");
        assert_eq!(
            res.outcome,
            Outcome::NoEstimate {
                reasons: vec![NoEstimateReason::ObservationTooShort]
            }
        );
        // An explicit sub observation needs that much audio beyond the search span.
        let r = noise(48_000 * 5, 2);
        f.push(48_000, &r, &r);
        let res = f
            .find(FindBand::Band(Band::Sub), Some(8.0), 0)
            .expect("valid config");
        assert!(matches!(res.outcome, Outcome::NoEstimate { .. }));
    }

    #[test]
    fn finds_a_delay_in_the_requested_band_and_tracks_there() {
        let fs = 48_000.0;
        let mut f = Finder::new(fs);
        let d = 120;
        let r = noise(48_000 * 4, 3);
        let m: Vec<f32> = std::iter::repeat_n(0.0, d)
            .chain(r.iter().copied())
            .take(r.len())
            .collect();
        f.push(0, &r, &m);
        let res = f
            .find(FindBand::Band(Band::Mid), None, 0)
            .expect("valid config");
        let pick = res.pick().expect("an estimate");
        assert!((pick.delay_frac - d as f64).abs() < 1.0, "{res:?}");
        assert_eq!(res.band, Band::Mid);
        assert_eq!(f.track_band, Band::Mid);
        // Observation: the mid band's default.
        assert_eq!(res.meas_window.end - res.meas_window.start, 24_000);
    }

    /// `r` through paths `(delay samples, gain)`, plus noise 30 dB down.
    fn paths(r: &[f32], arrivals: &[(usize, f32)], seed: u64) -> Vec<f32> {
        let n = noise(r.len(), seed);
        (0..r.len())
            .map(|i| {
                arrivals
                    .iter()
                    .filter(|(d, _)| i >= *d)
                    .map(|(d, g)| g * r[i - d])
                    .sum::<f32>()
                    + 0.03 * n[i]
            })
            .collect()
    }

    #[test]
    fn an_ambiguous_finding_pauses_tracking_until_resolved() {
        let fs = 48_000.0;
        let mut f = Finder::new(fs);
        f.track(true, 0);
        // A direct arrival 12.3 dB under the strongest: within 2 dB of the first-arrival
        // threshold, so the finder will not choose for the operator.
        let r = noise(48_000 * 4, 5);
        let gain = 10f32.powf(-12.3 / 20.0);
        let m = paths(&r, &[(300, gain), (500, 1.0)], 6);
        assert_eq!(f.push(0, &r, &m), None);
        let res = f
            .find(FindBand::Band(Band::FullRange), None, 0)
            .expect("valid config");
        assert!(
            matches!(res.outcome, Outcome::Ambiguous { .. }),
            "{:?}",
            res.outcome
        );
        assert!(f.paused());

        // The room changes to one clean arrival: tracking would lock onto it within two
        // windows, but it waits for the operator.
        let clean = |seed: u64| {
            let r = noise(48_000 * 2, seed);
            let m = paths(&r, &[(500, 1.0)], seed + 1);
            (r, m)
        };
        let mut start = r.len() as u64;
        for seed in [10, 20] {
            let (r, m) = clean(seed);
            assert_eq!(f.push(start, &r, &m), None, "paused tracking moved");
            start += r.len() as u64;
        }

        // The operator picks (insert / set): tracking resumes and follows the room again.
        f.set_held(300);
        f.set_paused(false);
        let mut moved = None;
        for seed in [30, 40] {
            let (r, m) = clean(seed);
            moved = moved.or(f.push(start, &r, &m));
            start += r.len() as u64;
        }
        assert_eq!(moved, Some(500));
    }

    #[test]
    fn a_new_finding_that_is_not_ambiguous_resumes_tracking() {
        let mut f = Finder::new(48_000.0);
        f.track(true, 0);
        f.set_paused(true);
        let r = noise(48_000 * 4, 7);
        let m = paths(&r, &[(120, 1.0)], 8);
        f.push(0, &r, &m);
        let res = f
            .find(FindBand::Band(Band::FullRange), None, 0)
            .expect("valid config");
        assert!(matches!(res.outcome, Outcome::Accepted { .. }), "{res:?}");
        assert!(!f.paused());
    }

    #[test]
    fn invalid_band_is_an_error() {
        let mut f = Finder::new(48_000.0);
        let bad = Band::Custom {
            lo_hz: 500.0,
            hi_hz: 100.0,
        };
        assert!(f.find(FindBand::Band(bad), None, 0).is_err());
    }
}
