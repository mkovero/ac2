//! Session input meters: peak, RMS and clip of every captured input while a session is open,
//! whether or not a measurement runs on it, so the operator sees which inputs carry signal
//! before choosing what to measure. They run on the fan-out thread, from the block
//! statistics it computes anyway, so they cost no thread and no rescan of the samples.

use std::time::{Duration, Instant};

use ac2_proto::frame::{FrameData, ProtectionFlags, SessionLevelsFrame};
use ac2_proto::topic::Topic;
use ac2_proto::units::Rev;

use super::{Emitter, LevelsMeter, Meters, StampArgs};
use crate::fanout::Block;

/// Meter frames per second at most. A meter read by eye gains nothing above ~30 updates per
/// second, and the 300 ms RMS barely moves between them.
pub(crate) const METER_FPS: u32 = 30;

/// Shortest interval between two meter frames.
pub(crate) fn meter_period() -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(METER_FPS))
}

pub(crate) struct SessionLevels {
    levels: LevelsMeter,
    config_rev: Rev,
    wall: u64,
    last: Option<Instant>,
}

impl SessionLevels {
    /// Meters every block channel of a session capturing `input_map`.
    pub(crate) fn new(input_map: &[u16], sample_rate: u32, config_rev: Rev) -> Self {
        Self {
            levels: LevelsMeter::new(
                (0..input_map.len()).collect(),
                input_map.to_vec(),
                sample_rate,
            ),
            config_rev,
            wall: 0,
            last: None,
        }
    }

    pub(crate) fn push(&mut self, b: &Block) {
        self.levels.push(b);
        self.wall = b.wall_ns;
    }

    /// Publishes a frame if one is due. `slack`: how early a frame may go, so a wakeup
    /// schedule close to the meter period does not drop every other frame.
    pub(crate) fn emit(&mut self, e: &Emitter, slack: Duration) {
        // Between frames the meter keeps accumulating: the next frame's peak covers the
        // whole gap, so no transient is lost to the rate limit.
        if self
            .last
            .is_some_and(|t| t.elapsed() + slack < meter_period())
        {
            return;
        }
        self.last = Some(Instant::now());
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
        if e.wants(Topic::SessionLevels) {
            e.send(
                stamp,
                FrameData::SessionLevels(SessionLevelsFrame {
                    meta,
                    peak,
                    rms,
                    clip,
                }),
            );
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
