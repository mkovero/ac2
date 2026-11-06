//! The capture clock's rate (`docs/design/multi-rig.md` §4), measured in the fan-out.
//!
//! The device's rate is fitted from the backend's block stamps, which are on the backend's
//! own clock. Where the stream can read that clock beside the host's disciplined one, a
//! second window of paired reads over the same minute measures the disciplined clock
//! against it, and the device's rate is said against the disciplined clock; elsewhere it is
//! said against the computer's own clock, never as the system clock's.

use std::sync::Arc;

use ac2_audio::StampClock;
use ac2_core::clock_rate::{ClockRate, RateWindow};
use ac2_proto::model::{ClockBasis, SampleClock};
use ac2_proto::units::Seconds;

/// The device's stamps are fitted over the last minute of audio, a point kept per 50 ms.
const WINDOW_S: f64 = 60.0;
const STEP_S: f64 = 0.05;
/// A pair of clock reads kept per 250 ms of disciplined time: each read is good to a
/// microsecond or so, so a minute of them reads the stamp clock to well under 0.1 ppm.
const PAIR_STEP_S: f64 = 0.25;
/// The pair window's index is disciplined time in ns.
const NS_PER_S: f64 = 1e9;

/// The capture clock's two windows.
pub(crate) struct CaptureClock {
    audio: RateWindow,
    stamps: Option<(Arc<dyn StampClock>, RateWindow)>,
}

impl CaptureClock {
    /// For a stream at `rate` Hz whose stamp clock is `stamps` (`None`: it cannot be read
    /// beside a disciplined clock).
    pub(crate) fn new(rate: f64, stamps: Option<Arc<dyn StampClock>>) -> Self {
        Self {
            audio: RateWindow::new(rate, WINDOW_S, STEP_S),
            stamps: stamps.map(|c| (c, RateWindow::new(NS_PER_S, WINDOW_S, PAIR_STEP_S))),
        }
    }

    /// A block's first frame `sample` was stamped `ns`; `breaks`: continuity broke before it.
    pub(crate) fn block(&mut self, sample: u64, ns: u64, breaks: bool) {
        self.audio.push(sample, ns, breaks);
    }

    /// Reads the stamp clock beside the disciplined one (the window keeps one pair per
    /// step, so this is called freely).
    pub(crate) fn pair(&mut self) {
        if let Some((clock, w)) = &mut self.stamps
            && let Some(p) = clock.pair()
        {
            w.push(p.disciplined_ns, p.stamp_ns, false);
        }
    }

    /// The reading now.
    pub(crate) fn reading(&self) -> SampleClock {
        reading(
            self.audio.rate(),
            self.stamps.as_ref().map(|(_, w)| w.rate()),
        )
    }
}

/// The capture clock's entry from the device's rate against the stamp clock (`None`: under
/// a window yet) and the disciplined clock's against the stamp clock (`None`: the stream
/// has no readable stamp clock; `Some(None)`: under a window yet).
fn reading(audio: Option<ClockRate>, reference: Option<Option<ClockRate>>) -> SampleClock {
    let measured = |r: ClockRate, against| SampleClock::Measured {
        ppm: r.ppm,
        span: Seconds(r.span_s),
        against,
    };
    match (audio, reference) {
        (Some(a), None) => measured(a, ClockBasis::HostCrystal),
        (Some(a), Some(Some(d))) => measured(a.against(d), ClockBasis::SystemClock),
        (None, _) | (Some(_), Some(None)) => SampleClock::Measuring,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(ppm: f64, nominal: f64) -> ClockRate {
        ClockRate {
            hz: nominal * (1.0 + ppm * 1e-6),
            ppm,
            span_s: 59.9,
        }
    }

    fn ppm(c: SampleClock) -> (f64, ClockBasis) {
        match c {
            SampleClock::Measured { ppm, against, .. } => (ppm, against),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_rate_is_said_against_the_clock_it_was_measured_against() {
        // No readable stamp clock: the figure is the device against the crystal.
        assert_eq!(
            ppm(reading(Some(rate(8.2, 48e3)), None)),
            (8.2, ClockBasis::HostCrystal)
        );
        // The disciplined clock reads 8.2 ppm fast against the stamp clock: the device's
        // +8.2 against the stamp clock is 0 against the disciplined clock.
        let (p, basis) = ppm(reading(Some(rate(8.2, 48e3)), Some(Some(rate(8.2, 1e9)))));
        assert!(p.abs() < 1e-4, "{p}");
        assert_eq!(basis, ClockBasis::SystemClock);
        // Either window short of a minute: no figure.
        assert_eq!(
            reading(Some(rate(8.2, 48e3)), Some(None)),
            SampleClock::Measuring
        );
        assert_eq!(reading(None, None), SampleClock::Measuring);
        assert_eq!(
            reading(None, Some(Some(rate(1.0, 1e9)))),
            SampleClock::Measuring
        );
    }
}
