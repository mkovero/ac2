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
    /// What the rigs take, read again only when a subscription has changed.
    taken: Taken,
    incarnation: StreamIncarnation,
    rate: u32,
    /// The blocks' host stamps are capture times on a clock the state maps to system time.
    capture_stamps: bool,
    scratch: Vec<f32>,
    /// One channel's samples as they go on the wire, kept between blocks.
    body: Vec<u8>,
    /// Frames the I/O thread's pipe did not take.
    dropped: u64,
}

/// The subscriptions as the tap last read them.
#[derive(Default)]
struct Taken {
    /// The [`Interest`] generation this was read at; `None` before the first read.
    generation: Option<u64>,
    /// Some rig takes audio.
    any: bool,
    /// Per block column: its channel is subscribed to.
    columns: Vec<bool>,
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
            taken: Taken::default(),
            incarnation,
            rate,
            capture_stamps,
            scratch: Vec::new(),
            body: Vec::new(),
            dropped: 0,
        }
    }

    /// Whether any rig takes audio now: the fan-out then hands off every device period.
    pub(crate) fn active(&mut self) -> bool {
        self.read_interest();
        self.taken.any
    }

    fn read_interest(&mut self) {
        // The count is read before the subscriptions, so what is kept under it is never
        // older than it.
        let generation = self.interest.generation();
        if self.taken.generation == Some(generation) {
            return;
        }
        self.taken.generation = Some(generation);
        self.taken.any = self.interest.any_under(b"audio/");
        self.taken.columns.clear();
        self.taken
            .columns
            .extend(self.columns.iter().map(|(_, t)| self.interest.wants(t)));
    }

    /// Sends `block`'s subscribed channels.
    pub(crate) fn send(&mut self, block: &Block) {
        self.read_interest();
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
        // The header is the same for every channel of the block: encoded once, by the
        // protocol's own encoder, when the first subscribed channel needs it.
        let mut head: Option<Vec<u8>> = None;
        for (col, (channel, topic)) in self.columns.iter().enumerate() {
            if !self.taken.columns[col] {
                continue;
            }
            if head.is_none() {
                head = encode_audio(*channel, &header, &[])
                    .ok()
                    .and_then(|parts| parts.into_iter().nth(1));
            }
            let Some(head) = &head else {
                return;
            };
            block.channel_into(col, &mut self.scratch);
            self.body.clear();
            self.body
                .extend(self.scratch.iter().flat_map(|s| s.to_le_bytes()));
            if !self.outbox.audio(topic, head, &self.body) {
                self.dropped += 1;
                if self.dropped.is_power_of_two() {
                    tracing::warn!("audio for other rigs: {} frame(s) not queued", self.dropped);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use ac2_proto::units::{DaemonIncarnation, SessionEpoch};
    use ac2_zmq::{Context, SocketType};

    use super::*;

    /// Each subscribed channel of a block reaches the I/O thread as the protocol's encoding
    /// of it behind the pipe's tag, block after block from the same buffers.
    #[test]
    fn frames_are_the_protocol_encoding() {
        let ctx = Context::new().unwrap();
        let pull = ctx.socket(SocketType::Pull).unwrap();
        pull.bind("inproc://tap-encoding").unwrap();
        let outbox = Outbox::connect(&ctx, "inproc://tap-encoding", 64).unwrap();
        let interest = Arc::new(Interest::default());
        let incarnation = StreamIncarnation {
            daemon: DaemonIncarnation(1),
            epoch: SessionEpoch(1),
        };
        let mut tap = SourceTap::new(outbox, interest.clone(), &[4, 7], incarnation, 96_000, true);
        interest.subscribe(b"audio/7");
        for start in [0_u64, 3] {
            let data: Vec<f32> = (0..6).map(|i| (start as f32) + 0.25 * i as f32).collect();
            let mut block = Block::new(start, 3, 2, BlockFlags::XRUN, 1_000 + start, data.clone());
            block.capture_stamp = true;
            tap.send(&block);
            let header = AudioHeader {
                v: PROTO_VERSION,
                start_sample: start,
                frames: 3,
                flags: AudioFlags::XRUN.with(AudioFlags::GAP),
                capture_ns: Some(1_000 + start),
                rate: 96_000,
                incarnation,
            };
            let column: Vec<f32> = data.iter().skip(1).step_by(2).copied().collect();
            let mut want = vec![vec![crate::outbox::TAG_AUDIO]];
            want.extend(encode_audio(7, &header, &column).unwrap());
            let got = pull.try_recv_parts().unwrap().unwrap();
            let got: Vec<Vec<u8>> = got.iter().map(|p| p.to_vec()).collect();
            assert_eq!(got, want);
            assert!(pull.try_recv_parts().unwrap().is_none());
        }
        // A subscription that changes is followed at the next block.
        assert!(tap.active());
        interest.unsubscribe(b"audio/7");
        assert!(!tap.active());
        interest.subscribe(b"audio/4");
        assert!(tap.active());
        tap.send(&Block::new(6, 1, 2, BlockFlags::NONE, 0, vec![0.5, 0.25]));
        let got = pull.try_recv_parts().unwrap().unwrap();
        assert_eq!(&*got[1], b"audio/4");
        assert_eq!(&*got[3], 0.5_f32.to_le_bytes());
        assert!(pull.try_recv_parts().unwrap().is_none());
    }
}
