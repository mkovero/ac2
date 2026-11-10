//! The source side (`docs/design/remote-inputs.md` §2.1): each captured channel another rig
//! subscribes to goes out as `audio/<channel>`, one frame per device block, raw `f32`.

use std::sync::Arc;

use ac2_audio::BlockFlags;
use ac2_proto::PROTO_VERSION;
use ac2_proto::frame::{AudioFlags, AudioHeader, encode_audio};
use ac2_proto::model::StreamIncarnation;

use crate::fanout::Block;
use crate::io::Interest;
use crate::outbox::Outbox;

/// What the fan-out needs to serve the session's channels to other rigs.
pub(crate) struct SourceTap {
    outbox: Outbox,
    interest: Arc<Interest>,
    /// Device input channel per block column, and its topic.
    columns: Vec<(u16, Vec<u8>)>,
    incarnation: StreamIncarnation,
    rate: u32,
    /// The blocks' host stamps are capture times on a clock the state maps to system time.
    capture_stamps: bool,
    scratch: Vec<f32>,
    /// Frames the I/O thread's pipe did not take.
    dropped: u64,
}

impl SourceTap {
    pub(crate) fn new(
        outbox: Outbox,
        interest: Arc<Interest>,
        input_map: &[u16],
        incarnation: StreamIncarnation,
        rate: u32,
        capture_stamps: bool,
    ) -> Self {
        Self {
            outbox,
            interest,
            columns: input_map
                .iter()
                .map(|&c| (c, ac2_proto::Topic::Audio { channel: c }.to_bytes()))
                .collect(),
            incarnation,
            rate,
            capture_stamps,
            scratch: Vec::new(),
            dropped: 0,
        }
    }

    /// Whether any rig takes audio now: the fan-out then hands off every device period.
    pub(crate) fn active(&self) -> bool {
        self.interest.any_under(b"audio/")
    }

    /// Sends `block`'s subscribed channels.
    pub(crate) fn send(&mut self, block: &Block) {
        let mut flags = AudioFlags::NONE;
        if block.flags.contains(BlockFlags::XRUN) {
            flags = flags.with(AudioFlags::XRUN);
        }
        if block.flags.breaks_continuity() {
            flags = flags.with(AudioFlags::GAP);
        }
        let header = AudioHeader {
            v: PROTO_VERSION,
            start_sample: block.start_sample,
            frames: block.frames,
            flags,
            capture_ns: (self.capture_stamps && block.capture_stamp).then_some(block.host_ns),
            rate: self.rate,
            incarnation: self.incarnation,
        };
        for (col, (channel, topic)) in self.columns.iter().enumerate() {
            if !self.interest.wants(topic) {
                continue;
            }
            block.channel_into(col, &mut self.scratch);
            let Ok(parts) = encode_audio(*channel, &header, &self.scratch) else {
                continue;
            };
            if !self.outbox.audio(parts) {
                self.dropped += 1;
                if self.dropped.is_power_of_two() {
                    tracing::warn!("audio for other rigs: {} frame(s) not queued", self.dropped);
                }
            }
        }
    }
}
