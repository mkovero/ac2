//! Stream input meters: peak, RMS and clip of every captured input while a stream is open,
//! whether or not a measurement runs on it, so the operator sees which inputs carry signal
//! before choosing what to measure. They run on the fan-out thread, from the block
//! statistics it computes anyway, so they cost no thread and no rescan of the samples. The
//! programme meters ([`super::programme`]) ride along and share the frame.

use std::time::{Duration, Instant};

use ac2_proto::frame::{
    ClipFlags, FrameData, LevelsMark, METER_FPS, ProtectionFlags, SourceLevelsFrame,
    SourceLevelsMeta, StreamLevelsFrame, StreamLevelsMeta,
};
use ac2_proto::topic::Topic;
use ac2_proto::units::Rev;

use super::programme::Programme;
use super::{Emitter, LevelsMeter, Meters, StampArgs};
use crate::cadence::Cadence;
use crate::fanout::Block;

/// Shortest interval between two meter frames.
pub(crate) fn meter_period() -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(METER_FPS))
}

/// Meter frames a `source/levels` frame spans.
const SOURCE_SPAN: u32 = 2;

/// Interval between two `source/levels` frames: [`SOURCE_SPAN`] meter periods. Another rig
/// fits the cross-rig rate on their marks at a point per 50 ms
/// (`remote::join::CROSS_STEP_S`); a mark every 67 ms gives that fit a point each, and the
/// slope's error falls with the span, not with the point count.
pub(crate) fn source_period() -> Duration {
    meter_period() * SOURCE_SPAN
}

/// A rig's own meters over the meter frames since its last `source/levels` frame, so a
/// transient in any of them still reaches the other rig.
#[derive(Default)]
struct SourceHeld {
    /// Peak, true peak and the ballistic slot, the highest; clip, any; RMS as power summed
    /// over `n` frames.
    peak: Vec<f32>,
    true_peak: Vec<f32>,
    ppm: Vec<f32>,
    clip: Vec<ClipFlags>,
    power: Vec<f32>,
    n: u32,
}

/// One meter frame's columns that a `source/levels` frame holds over its span.
struct Interval<'a> {
    peak: &'a [f32],
    true_peak: &'a [f32],
    ppm: &'a [f32],
    clip: &'a [ClipFlags],
    rms: &'a [f32],
}

impl SourceHeld {
    fn add(&mut self, i: Interval<'_>) {
        // Meter frames are close to one period each, so RMS over the span is the mean of
        // their powers.
        let power = i.rms.iter().map(|r| 10f32.powf(r / 10.0));
        if self.n == 0 || self.peak.len() != i.peak.len() {
            self.peak = i.peak.to_vec();
            self.true_peak = i.true_peak.to_vec();
            self.ppm = i.ppm.to_vec();
            self.clip = i.clip.to_vec();
            self.power = power.collect();
            self.n = 1;
            return;
        }
        let high = |held: &mut Vec<f32>, new: &[f32]| {
            for (a, b) in held.iter_mut().zip(new) {
                *a = if a.is_nan() || b.is_nan() {
                    f32::NAN
                } else {
                    a.max(*b)
                };
            }
        };
        high(&mut self.peak, i.peak);
        high(&mut self.true_peak, i.true_peak);
        // The ballistic slot is the interval's highest reading (for the peak types a raw
        // peak with no decay of its own): the span's is the highest of its intervals.
        high(&mut self.ppm, i.ppm);
        self.clip
            .iter_mut()
            .zip(i.clip)
            .for_each(|(a, b)| *a = a.with(*b));
        self.power.iter_mut().zip(power).for_each(|(a, b)| *a += b);
        self.n += 1;
    }

    /// Whether the held meters span a whole `source/levels` frame.
    fn full(&self) -> bool {
        self.n >= SOURCE_SPAN
    }

    /// A meter frame's interval, `wanted`: whether `source/levels` is read now; the frame's
    /// columns once they span a whole one.
    fn offer(&mut self, wanted: bool, i: Interval<'_>) -> Option<Held> {
        if !wanted {
            // Nobody reads them: what was held belongs to no frame, and must not merge into
            // the first of the next subscription.
            *self = Self::default();
            return None;
        }
        self.add(i);
        self.full().then(|| self.take())
    }

    /// The held meters, starting again.
    fn take(&mut self) -> Held {
        let n = self.n.max(1) as f32;
        let rms = self.power.iter().map(|p| 10.0 * (p / n).log10()).collect();
        let h = std::mem::take(self);
        Held {
            peak: h.peak,
            true_peak: h.true_peak,
            ppm: h.ppm,
            clip: h.clip,
            rms,
        }
    }
}

/// What a `source/levels` frame carries of the meter frames it spans.
struct Held {
    peak: Vec<f32>,
    true_peak: Vec<f32>,
    ppm: Vec<f32>,
    clip: Vec<ClipFlags>,
    rms: Vec<f32>,
}

pub(crate) struct StreamLevels {
    levels: LevelsMeter,
    /// The remote inputs' meters, from the joined feed (its columns after the local ones).
    remote: Option<LevelsMeter>,
    programme: Programme,
    sample_rate: u32,
    config_rev: Rev,
    wall: u64,
    cadence: Cadence,
    held: SourceHeld,
}

impl StreamLevels {
    /// Meters every block channel of a stream capturing `input_map`. `early`: how far
    /// ahead of its slot a frame may go, so a wakeup just before a slot takes it.
    pub(crate) fn new(
        input_map: &[u16],
        remote: &[u16],
        sample_rate: u32,
        config_rev: Rev,
        early: Duration,
        programme: Programme,
    ) -> Self {
        Self {
            levels: LevelsMeter::new(
                (0..input_map.len()).collect(),
                input_map.to_vec(),
                sample_rate,
            ),
            remote: (!remote.is_empty()).then(|| {
                let n = input_map.len();
                LevelsMeter::new(
                    (n..n + remote.len()).collect(),
                    remote.to_vec(),
                    sample_rate,
                )
            }),
            programme,
            sample_rate,
            config_rev,
            wall: 0,
            cadence: Cadence::new(meter_period(), early),
            held: SourceHeld::default(),
        }
    }

    pub(crate) fn push(&mut self, b: &Block) {
        self.levels.push(b);
        self.programme.push(b);
        self.wall = b.wall_ns;
    }

    /// A block of the joined feed.
    pub(crate) fn push_remote(&mut self, b: &Block) {
        if let Some(r) = &mut self.remote {
            r.push(b);
        }
    }

    /// Publishes a frame if one is due; `mark`: the newest stamped block on system time,
    /// carried on the frame other rigs read (`source/levels`).
    pub(crate) fn emit(&mut self, e: &Emitter, mark: Option<LevelsMark>) {
        // Between frames the meter keeps accumulating: the next frame's peak covers the
        // whole gap, so no transient is lost to the rate limit.
        let now = Instant::now();
        if !self.cadence.is_ready(now) {
            return;
        }
        self.cadence.take(now);
        let stamp = meter_stamp(&self.levels, self.config_rev, self.wall);
        let Some(Meters {
            meta,
            peak,
            rms,
            clip,
        }) = self.levels.take_meters()
        else {
            return;
        };
        let gonio = e.wants(Topic::StreamGonio);
        let mut r = self.programme.take(gonio);
        // Another rig reads this rig's own inputs only, published whether or not an app
        // watches: its clocks are related to this one's through the mark.
        let interval = Interval {
            peak: &peak,
            true_peak: &r.true_peak,
            ppm: &r.ppm,
            clip: &clip,
            rms: &rms,
        };
        if let Some(h) = self.held.offer(e.wants(Topic::SourceLevels), interval) {
            // `max` and `overs` hold since a reset, and the pairs' and sources' loudness are
            // running readings: the newest frame's say all of the span.
            e.send(
                stamp,
                FrameData::SourceLevels(SourceLevelsFrame {
                    meta: SourceLevelsMeta {
                        channels: meta.channels.clone(),
                        ballistic: r.ballistic,
                        pairs: r.pairs.clone(),
                        sources: r.sources.clone(),
                        sample_rate_hz: self.sample_rate,
                        mark,
                    },
                    pair_levels: r.pair_levels.clone(),
                    loudness: r.loudness.clone(),
                    peak: h.peak,
                    rms: h.rms,
                    clip: h.clip,
                    ppm: h.ppm,
                    true_peak: h.true_peak,
                    max: r.max.clone(),
                    overs: r.overs.clone(),
                }),
            );
        }
        let (mut channels, mut peak, mut rms, mut clip) = (meta.channels, peak, rms, clip);
        // The programme meters run on the local inputs; a remote input's ballistic reads its
        // sample peak, the one peak its block statistics give. Its true peak and maximum
        // are not metered: a sample peak would read low as either.
        if let Some(m) = self.remote.as_mut().and_then(LevelsMeter::take_meters) {
            let n = m.peak.len();
            channels.extend(m.meta.channels);
            r.ppm.extend(&m.peak);
            r.true_peak.extend(std::iter::repeat_n(f32::NAN, n));
            r.max.extend(std::iter::repeat_n(f32::NAN, n));
            r.overs.extend(std::iter::repeat_n(0, n));
            peak.extend(m.peak);
            rms.extend(m.rms);
            clip.extend(m.clip);
        }
        if e.wants(Topic::StreamLevels) {
            e.send(
                stamp,
                FrameData::StreamLevels(StreamLevelsFrame {
                    meta: StreamLevelsMeta {
                        channels,
                        ballistic: r.ballistic,
                        pairs: r.pairs,
                        sources: r.sources,
                    },
                    pair_levels: r.pair_levels,
                    loudness: r.loudness,
                    peak,
                    rms,
                    clip,
                    ppm: r.ppm,
                    true_peak: r.true_peak,
                    max: r.max,
                    overs: r.overs,
                }),
            );
        }
        if let Some(g) = r.gonio {
            e.send(stamp, FrameData::StreamGonio(g));
        }
    }
}

/// Header fields of a meter frame ending at `end`.
pub(crate) fn meter_stamp(levels: &LevelsMeter, config_rev: Rev, wall_ns: u64) -> StampArgs {
    StampArgs {
        audio_sample: levels.end().saturating_sub(1),
        config_rev,
        applied_at: 0,
        wall_ns,
        grid_id: None,
        protection: if levels.any_clip_held() {
            ProtectionFlags::CLIP
        } else {
            ProtectionFlags::NONE
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interval<'a>(
        peak: &'a [f32],
        true_peak: &'a [f32],
        ppm: &'a [f32],
        clip: &'a [ClipFlags],
        rms: &'a [f32],
    ) -> Interval<'a> {
        Interval {
            peak,
            true_peak,
            ppm,
            clip,
            rms,
        }
    }

    /// Two meter frames into one `source/levels` frame: the higher peak, true peak and
    /// ballistic slot, either clip, and the RMS of the two intervals together.
    #[test]
    fn a_source_frame_holds_the_meter_frames_it_spans() {
        let mut h = SourceHeld::default();
        // A click in the first interval, silence in the second: under a true-peak ballistic
        // the slot is the interval's raw peak, so the second alone would read −40.
        h.add(interval(
            &[-6.0],
            &[-3.0],
            &[-3.0],
            &[ClipFlags::NONE],
            &[-20.0],
        ));
        assert!(!h.full());
        h.add(interval(
            &[-12.0],
            &[-40.0],
            &[-40.0],
            &[ClipFlags::CLIP],
            &[f32::NEG_INFINITY],
        ));
        assert!(h.full());
        let t = h.take();
        assert_eq!(
            (t.peak, t.true_peak, t.ppm, t.clip),
            (vec![-6.0], vec![-3.0], vec![-3.0], vec![ClipFlags::CLIP])
        );
        // Half the time at −20 dBFS, half silent: 3 dB down.
        assert!((t.rms[0] - (-20.0 - 3.0103)).abs() < 1e-3, "{:?}", t.rms);
        // Taken: the next frame starts afresh.
        h.add(interval(
            &[-30.0],
            &[-29.0],
            &[-29.0],
            &[ClipFlags::NONE],
            &[-40.0],
        ));
        assert!(!h.full());
        let t = h.take();
        assert_eq!((t.peak, t.clip), (vec![-30.0], vec![ClipFlags::NONE]));
        assert!((t.rms[0] + 40.0).abs() < 1e-4, "{:?}", t.rms);
    }

    /// A clip held when the topic stops being read does not reach the first frame after
    /// it is read again.
    #[test]
    fn what_was_held_goes_when_nobody_reads_source_levels() {
        let mut h = SourceHeld::default();
        let loud = || interval(&[0.0], &[0.5], &[0.5], &[ClipFlags::CLIP], &[-3.0]);
        let quiet = || interval(&[-30.0], &[-29.0], &[-29.0], &[ClipFlags::NONE], &[-40.0]);
        assert!(h.offer(true, loud()).is_none());
        assert!(h.offer(false, quiet()).is_none());
        assert!(h.offer(true, quiet()).is_none());
        let t = h.offer(true, quiet()).expect("a whole span");
        assert_eq!((t.peak, t.clip), (vec![-30.0], vec![ClipFlags::NONE]));
    }
}
