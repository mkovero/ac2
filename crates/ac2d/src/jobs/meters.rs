//! Session input meters: peak, RMS and clip of every captured input while a session is open,
//! whether or not a measurement runs on it, so the operator sees which inputs carry signal
//! before choosing what to measure. They run on the fan-out thread, from the block
//! statistics it computes anyway, so they cost no thread and no rescan of the samples. The
//! programme meters ([`super::programme`]) ride along and share the frame.

use std::time::{Duration, Instant};

use ac2_proto::frame::{
    FrameData, METER_FPS, ProtectionFlags, SessionLevelsFrame, SessionLevelsMeta,
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

pub(crate) struct SessionLevels {
    levels: LevelsMeter,
    programme: Programme,
    config_rev: Rev,
    wall: u64,
    cadence: Cadence,
}

impl SessionLevels {
    /// Meters every block channel of a session capturing `input_map`. `early`: how far
    /// ahead of its slot a frame may go, so a wakeup just before a slot takes it.
    pub(crate) fn new(
        input_map: &[u16],
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
            programme,
            config_rev,
            wall: 0,
            cadence: Cadence::new(meter_period(), early),
        }
    }

    pub(crate) fn push(&mut self, b: &Block) {
        self.levels.push(b);
        self.programme.push(b);
        self.wall = b.wall_ns;
    }

    /// Publishes a frame if one is due.
    pub(crate) fn emit(&mut self, e: &Emitter) {
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
        let r = self.programme.take(gonio);
        if e.wants(Topic::SessionLevels) {
            e.send(
                stamp,
                FrameData::SessionLevels(SessionLevelsFrame {
                    meta: SessionLevelsMeta {
                        channels: meta.channels,
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
