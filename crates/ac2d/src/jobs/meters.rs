//! Session input meters: peak, RMS and clip of every captured input while a session is open,
//! whether or not a measurement runs on it, so the operator sees which inputs carry signal
//! before choosing what to measure. They run on the fan-out thread, from the block
//! statistics it computes anyway, so they cost no thread and no rescan of the samples. The
//! programme meters ([`super::programme`]) ride along and share the frame.

use std::time::{Duration, Instant};

use ac2_proto::frame::{
    ClipFlags, FrameData, LevelsMark, METER_FPS, ProtectionFlags, SessionLevelsFrame,
    SessionLevelsMeta, SourceLevelsFrame, SourceLevelsMeta,
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

/// Interval between two `source/levels` frames: two meter periods. Another rig fits the
/// cross-rig rate on their marks keeping one per 50 ms (`remote::join::CROSS_STEP_S`), so
/// at the meter rate it kept every second mark anyway; at half the rate it keeps every one,
/// the same points, and the link carries half the bytes.
pub(crate) fn source_period() -> Duration {
    meter_period() * 2
}

/// A rig's own meters over the meter frames since its last `source/levels` frame, so a
/// transient between two of them still reaches the other rig.
#[derive(Default)]
struct SourceHeld {
    /// Peak and true peak, the highest; clip, any; RMS as power summed over `n` frames.
    peak: Vec<f32>,
    true_peak: Vec<f32>,
    clip: Vec<ClipFlags>,
    power: Vec<f32>,
    n: u32,
}

impl SourceHeld {
    fn add(&mut self, peak: &[f32], true_peak: &[f32], clip: &[ClipFlags], rms: &[f32]) {
        // Meter frames are close to one period each, so RMS over two is the mean of their
        // powers.
        let power = rms.iter().map(|r| 10f32.powf(r / 10.0));
        if self.n == 0 || self.peak.len() != peak.len() {
            self.peak = peak.to_vec();
            self.true_peak = true_peak.to_vec();
            self.clip = clip.to_vec();
            self.power = power.collect();
            self.n = 1;
            return;
        }
        let high = |a: &mut f32, b: f32| {
            *a = if a.is_nan() || b.is_nan() {
                f32::NAN
            } else {
                a.max(b)
            }
        };
        self.peak
            .iter_mut()
            .zip(peak)
            .for_each(|(a, b)| high(a, *b));
        self.true_peak
            .iter_mut()
            .zip(true_peak)
            .for_each(|(a, b)| high(a, *b));
        self.clip
            .iter_mut()
            .zip(clip)
            .for_each(|(a, b)| *a = a.with(*b));
        self.power.iter_mut().zip(power).for_each(|(a, b)| *a += b);
        self.n += 1;
    }

    /// The held meters (peak, true peak, clip, RMS dBFS), starting again.
    fn take(&mut self) -> (Vec<f32>, Vec<f32>, Vec<ClipFlags>, Vec<f32>) {
        let n = self.n.max(1) as f32;
        self.n = 0;
        let rms = self.power.iter().map(|p| 10.0 * (p / n).log10()).collect();
        (
            std::mem::take(&mut self.peak),
            std::mem::take(&mut self.true_peak),
            std::mem::take(&mut self.clip),
            rms,
        )
    }
}

pub(crate) struct SessionLevels {
    levels: LevelsMeter,
    /// The remote inputs' meters, from the joined feed (its columns after the local ones).
    remote: Option<LevelsMeter>,
    programme: Programme,
    sample_rate: u32,
    config_rev: Rev,
    wall: u64,
    cadence: Cadence,
    source: Cadence,
    held: SourceHeld,
}

impl SessionLevels {
    /// Meters every block channel of a session capturing `input_map`. `early`: how far
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
            source: Cadence::new(source_period(), early),
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
        let gonio = e.wants(Topic::SessionGonio);
        let mut r = self.programme.take(gonio);
        // Another rig reads this rig's own inputs only, published whether or not an app
        // watches: its clocks are related to this one's through the mark.
        if e.wants(Topic::SourceLevels) {
            self.held.add(&peak, &r.true_peak, &clip, &rms);
            if self.source.is_ready(now) {
                self.source.take(now);
                // The ballistic reading carries its own decay and `max` and `overs` hold
                // since a reset: the newest frame's say all of the interval.
                let (peak, true_peak, clip, rms) = self.held.take();
                e.send(
                    stamp,
                    FrameData::SourceLevels(SourceLevelsFrame {
                        meta: SourceLevelsMeta {
                            channels: meta.channels.clone(),
                            ballistic: r.ballistic,
                            pair_levels: r.pair_levels.clone(),
                            loudness: r.loudness.clone(),
                            sample_rate_hz: self.sample_rate,
                            mark,
                        },
                        peak,
                        rms,
                        clip,
                        ppm: r.ppm.clone(),
                        true_peak,
                        max: r.max.clone(),
                        overs: r.overs.clone(),
                    }),
                );
            }
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
        if e.wants(Topic::SessionLevels) {
            e.send(
                stamp,
                FrameData::SessionLevels(SessionLevelsFrame {
                    meta: SessionLevelsMeta {
                        channels,
                        ballistic: r.ballistic,
                        pair_levels: r.pair_levels,
                        loudness: r.loudness,
                    },
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
            e.send(stamp, FrameData::SessionGonio(g));
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

    /// Two meter frames into one `source/levels` frame: the higher peak, either clip, and
    /// the RMS of the two intervals together.
    #[test]
    fn a_source_frame_holds_the_meter_frames_it_spans() {
        let mut h = SourceHeld::default();
        h.add(&[-6.0], &[-5.5], &[ClipFlags::NONE], &[-20.0]);
        h.add(&[-12.0], &[-11.0], &[ClipFlags::CLIP], &[f32::NEG_INFINITY]);
        let (peak, true_peak, clip, rms) = h.take();
        assert_eq!(
            (peak, true_peak, clip),
            (vec![-6.0], vec![-5.5], vec![ClipFlags::CLIP])
        );
        // Half the time at −20 dBFS, half silent: 3 dB down.
        assert!((rms[0] - (-20.0 - 3.0103)).abs() < 1e-3, "{rms:?}");
        // Taken: the next frame starts afresh.
        h.add(&[-30.0], &[-29.0], &[ClipFlags::NONE], &[-40.0]);
        let (peak, _, clip, rms) = h.take();
        assert_eq!((peak, clip), (vec![-30.0], vec![ClipFlags::NONE]));
        assert!((rms[0] + 40.0).abs() < 1e-4, "{rms:?}");
    }
}
