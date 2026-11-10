use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use ac2_audio::BlockFlags;

use ac2_proto::PROTO_VERSION;
use ac2_proto::frame::AudioHeader;
use ac2_proto::model::{CalStatus, Clocks, SourceInput, SourceSession, StampMap};
use ac2_proto::units::DaemonIncarnation;

use super::*;

const RATE: u32 = 48_000;
const BLOCK: u32 = 64;
/// The remote rig's sample index runs this far ahead of this rig's.
const AHEAD: u64 = 5000;
/// The default link offset at 64 frames on both rigs: 2·64 + 64 frames and 2 ms.
const LINK: u64 = 3 * 64 + 96;

fn value(remote_sample: u64) -> f32 {
    (remote_sample % 997) as f32 / 997.0
}

fn stamp(local_sample: u64) -> u64 {
    (local_sample as f64 * 1e9 / f64::from(RATE)).round() as u64
}

const INC: StreamIncarnation = StreamIncarnation {
    daemon: DaemonIncarnation(7),
    epoch: SessionEpoch(1),
};

fn source(map_offset_ns: u64) -> Box<SourceState> {
    Box::new(SourceState {
        rig: RigId(2),
        name: "ketunkolo".into(),
        clocks: Clocks {
            system: SystemClock::Synced { maxerror_us: 100 },
            capture: None,
            stamp_map: Some(StampMap {
                epoch_stamp_ns: 0,
                system_ns: map_offset_ns,
                ppm: 0.0,
            }),
        },
        session: Some(SourceSession {
            incarnation: INC,
            sample_rate_hz: RATE,
            buffer_frames: BLOCK,
            replaying: false,
            capture_stamps: true,
            inputs: vec![SourceInput {
                channel: 3,
                setup: None,
                cal_status: CalStatus::Uncalibrated,
                calibration: None,
                curve: None,
            }],
        }),
    })
}

struct Rig {
    joiner: Joiner,
    link: SyncSender<LinkFrame>,
    control: Receiver<ControlMsg>,
    /// Next local block start.
    next: u64,
    stamps: bool,
}

impl Rig {
    /// A joiner whose one remote input's rig has stated its source.
    fn new(stamps: bool) -> Self {
        let r = Self::bare(stamps, &[3]);
        r.link
            .try_send(LinkFrame::Source(source(0)))
            .expect("queued");
        r
    }

    /// A joiner over remote `channels` of one rig, nothing read from it yet.
    fn bare(stamps: bool, channels: &[u16]) -> Self {
        let (link, rx) = sync_channel(1024);
        let (to_control, control) = std::sync::mpsc::channel();
        let joiner = Joiner::new(JoinSetup {
            inputs: channels
                .iter()
                .enumerate()
                .map(|(slot, &channel)| JoinInput {
                    slot: slot as u16,
                    rig: RigId(2),
                    channel,
                })
                .collect(),
            rigs: vec![(RigId(2), rx)],
            local_columns: 1,
            rate: RATE,
            period: BLOCK,
            link_offset: None,
            system_synced: Arc::new(AtomicBool::new(true)),
            to_control,
            epoch: SessionEpoch(1),
        });
        Self {
            joiner,
            link,
            control,
            next: 0,
            stamps,
        }
    }

    /// The remote frame captured at local sample `local`, `AHEAD` samples later on its rig.
    fn send(&self, local: u64, rate: u32) {
        self.send_on(3, local, rate);
    }

    fn send_on(&self, channel: u16, local: u64, rate: u32) {
        let start = local + AHEAD;
        self.link
            .try_send(LinkFrame::Audio(AudioFrame {
                channel,
                header: AudioHeader {
                    v: PROTO_VERSION,
                    start_sample: start,
                    frames: BLOCK,
                    flags: AudioFlags::NONE,
                    capture_ns: self.stamps.then(|| stamp(local)),
                    rate,
                    incarnation: INC,
                },
                samples: (start..start + u64::from(BLOCK)).map(value).collect(),
            }))
            .expect("queued");
    }

    /// One local block; the joined blocks now due.
    fn push(&mut self, now: Instant) -> Vec<Block> {
        let mut b = Block::new(
            self.next,
            BLOCK,
            1,
            BlockFlags::NONE,
            0,
            vec![0.25; BLOCK as usize],
        );
        b.host_ns = stamp(self.next);
        b.capture_stamp = self.stamps;
        self.next += u64::from(BLOCK);
        let line = StampLine {
            epoch_stamp_ns: 0,
            system_ns: 0,
            ppm: 0.0,
        };
        self.joiner.push(&[b], Some(line), now)
    }

    fn last_report(&self) -> Option<(Vec<InputReport>, Vec<RigReport>)> {
        let mut last = None;
        while let Ok(m) = self.control.try_recv() {
            if let ControlMsg::Remote(RemoteMsg::Joined { inputs, rigs, .. }) = m {
                last = Some((inputs, rigs));
            }
        }
        last
    }
}

/// Every joined frame's remote sample is the one captured at its local index.
fn assert_joined(blocks: &[Block]) {
    for b in blocks {
        assert_eq!(b.channels, 2);
        assert!(b.marks.get(1).is_none(), "gap at {}", b.start_sample);
        for f in 0..b.frames as usize {
            let at = b.start_sample + f as u64;
            assert_eq!(b.data[2 * f], 0.25);
            assert_eq!(b.data[2 * f + 1], value(at + AHEAD), "sample {at}");
        }
    }
}

#[test]
fn audio_on_time_joins_at_its_own_index_a_link_offset_late() {
    let mut r = Rig::new(false);
    r.link
        .try_send(LinkFrame::Source(source(0)))
        .expect("queued");
    let t0 = Instant::now();
    let mut out = Vec::new();
    for _ in 0..40 {
        let at = r.next;
        r.send(at, RATE);
        out.extend(r.push(t0));
    }
    assert!(!out.is_empty());
    assert_eq!(out[0].start_sample, 0);
    // Held back by the link offset, no more.
    let released = out.last().map_or(0, Block::end_sample);
    assert!(r.next - released <= LINK + u64::from(BLOCK), "{released}");
    assert_joined(&out);
    r.push(t0 + Duration::from_secs(2));
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(inputs[0].audio, RemoteAudio::Streaming);
    assert_eq!(
        inputs[0].anchor,
        Some(Anchor::Arrival {
            reason: ArrivalReason::NoCaptureStamps
        })
    );
    assert_eq!(inputs[0].late_frames, 0);
}

#[test]
fn capture_stamps_anchor_on_system_time_whatever_the_transit() {
    let mut r = Rig::new(true);
    r.link
        .try_send(LinkFrame::Source(source(0)))
        .expect("queued");
    let t0 = Instant::now();
    let mut out = Vec::new();
    // Each remote block arrives three of this rig's blocks after its capture.
    for _ in 0..3 {
        out.extend(r.push(t0));
    }
    for _ in 0..40 {
        let at = r.next - 3 * u64::from(BLOCK);
        r.send(at, RATE);
        out.extend(r.push(t0));
    }
    // The blocks released before the first remote audio came lack it; from there on the
    // samples pair by capture time.
    let joined: Vec<Block> = out.into_iter().filter(|b| b.start_sample >= 192).collect();
    assert!(!joined.is_empty());
    assert_joined(&joined);
    r.push(t0 + Duration::from_secs(2));
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(inputs[0].anchor, Some(Anchor::SystemTime));
}

#[test]
fn system_clocks_apart_by_more_than_the_link_offset_anchor_on_arrival() {
    let mut r = Rig::new(true);
    // The source's system clock is 50 ms ahead of this rig's.
    r.link
        .try_send(LinkFrame::Source(source(50_000_000)))
        .expect("queued");
    let t0 = Instant::now();
    for _ in 0..10 {
        let at = r.next;
        r.send(at, RATE);
        r.push(t0);
    }
    r.push(t0 + Duration::from_secs(2));
    let (inputs, rigs) = r.last_report().expect("reported");
    match inputs[0].anchor {
        Some(Anchor::Arrival {
            reason: ArrivalReason::ClocksDisagree { by },
        }) => assert!((by.0 - 0.05).abs() < 0.002, "{by:?}"),
        a => panic!("{a:?}"),
    }
    assert!(rigs[0].disagree.is_some());
}

#[test]
fn missing_audio_is_a_remote_gap_and_late_audio_is_counted() {
    let mut r = Rig::new(false);
    let t0 = Instant::now();
    let mut out = Vec::new();
    for i in 0..30 {
        // Blocks 10 … 14 never come.
        if !(10..15).contains(&i) {
            let at = r.next;
            r.send(at, RATE);
        }
        out.extend(r.push(t0));
    }
    let gaps: Vec<u64> = out
        .iter()
        .filter(|b| !b.marks.gaps(1).is_empty())
        .map(|b| b.start_sample)
        .collect();
    assert_eq!(gaps, (10..15).map(|i| i * 64).collect::<Vec<_>>());
    // A frame for audio already released is late.
    r.send(0, RATE);
    r.push(t0 + Duration::from_secs(2));
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(inputs[0].late_frames, 1);
}

#[test]
fn audio_far_ahead_of_the_present_slips_by_a_known_count() {
    let mut r = Rig::new(false);
    let t0 = Instant::now();
    let mut out = Vec::new();
    for _ in 0..10 {
        let at = r.next;
        r.send(at, RATE);
        out.extend(r.push(t0));
    }
    // The source's index jumps a second ahead without a new incarnation.
    for _ in 0..20 {
        let at = r.next + u64::from(RATE);
        r.send(at, RATE);
        out.extend(r.push(t0));
    }
    let slips: Vec<i64> = out
        .iter()
        .filter_map(|b| b.marks.get(1).map(|m| m.slip))
        .filter(|&s| s != 0)
        .collect();
    assert_eq!(slips.len(), 1, "{slips:?}");
    r.push(t0 + Duration::from_secs(2));
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(inputs[0].slips, 1);
    assert_eq!(inputs[0].last_slip, Some(slips[0]));
    assert!(slips[0] < -i64::from(RATE) / 2, "{slips:?}");
}

#[test]
fn a_source_at_another_rate_or_replaying_gives_no_audio() {
    let mut r = Rig::new(false);
    let t0 = Instant::now();
    r.send(0, 96_000);
    let out = r.push(t0 + Duration::from_secs(1));
    assert!(out.is_empty());
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(
        inputs[0].audio,
        RemoteAudio::RateMismatch {
            theirs: 96_000,
            ours: RATE
        }
    );
    let mut s = source(0);
    if let Some(ss) = &mut s.session {
        ss.replaying = true;
    }
    r.link.try_send(LinkFrame::Source(s)).expect("queued");
    r.push(t0 + Duration::from_secs(3));
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(inputs[0].audio, RemoteAudio::Replaying);
}

/// One input's missing audio marks its own column only: a job reading the other input, or
/// this rig's, carries on.
#[test]
fn a_gap_marks_only_the_input_it_is_in() {
    let mut s = source(0);
    if let Some(ss) = &mut s.session {
        let first = ss.inputs[0].clone();
        ss.inputs.push(SourceInput {
            channel: 4,
            ..first
        });
    }
    let mut r = Rig::bare(false, &[3, 4]);
    r.link.try_send(LinkFrame::Source(s)).expect("queued");
    let t0 = Instant::now();
    let mut out = Vec::new();
    for i in 0..30 {
        let at = r.next;
        r.send_on(3, at, RATE);
        // Channel 4 misses blocks 10 … 14.
        if !(10..15).contains(&i) {
            r.send_on(4, at, RATE);
        }
        out.extend(r.push(t0));
    }
    let gapped: Vec<u64> = out
        .iter()
        .filter(|b| b.restarts_averages(&[2]))
        .map(|b| b.start_sample)
        .collect();
    assert_eq!(gapped, (10..15).map(|i| i * 64).collect::<Vec<_>>());
    assert!(out.iter().all(|b| !b.restarts_averages(&[0, 1])));
    // The missing samples are whole-block runs on column 2.
    let b = out
        .iter()
        .find(|b| b.start_sample == 640)
        .expect("block 10");
    assert_eq!(b.marks.gaps(2).to_vec(), vec![0..BLOCK]);
}

/// Audio that keeps arriving after it was needed for a second runs the ring dry: the map
/// slips once, marked on that input's column with its count, and the audio lands again.
#[test]
fn audio_late_for_a_second_slips_the_map_once() {
    let mut r = Rig::new(false);
    r.next = 100_000;
    let t0 = Instant::now();
    let mut out = Vec::new();
    for _ in 0..10 {
        let at = r.next;
        r.send(at, RATE);
        out.extend(r.push(t0));
    }
    // The source falls 1000 samples behind: every frame now comes after its release.
    for j in 0..60u64 {
        let at = r.next - 1000;
        r.send(at, RATE);
        out.extend(r.push(t0 + Duration::from_millis(25 * j)));
    }
    let slips: Vec<i64> = out
        .iter()
        .filter_map(|b| b.marks.get(1).map(|m| m.slip))
        .filter(|&s| s != 0)
        .collect();
    assert_eq!(slips.len(), 1, "{slips:?}");
    assert!(slips[0] > 1000 - LINK as i64, "{slips:?}");
    // After the slip the audio is on time: the newest blocks are whole.
    assert!(out.iter().rev().take(5).all(|b| b.marks.get(1).is_none()));
    r.push(t0 + Duration::from_secs(3));
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(inputs[0].slips, 1);
    assert_eq!(inputs[0].last_slip, Some(slips[0]));
    assert!(inputs[0].late_frames > 30);
    assert_eq!(u64::from(inputs[0].late_frames_hour), inputs[0].late_frames);
}

/// Audio before the source's state is read waits for it, so the anchor can still be system
/// time rather than settling for arrival.
#[test]
fn audio_before_the_source_state_waits_to_anchor() {
    let mut r = Rig::bare(true, &[3]);
    let t0 = Instant::now();
    for _ in 0..10 {
        let at = r.next;
        r.send(at, RATE);
        r.push(t0);
    }
    r.push(t0 + Duration::from_secs(1));
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(inputs[0].audio, RemoteAudio::Starting);
    assert_eq!(inputs[0].anchor, None);

    r.link
        .try_send(LinkFrame::Source(source(0)))
        .expect("queued");
    let mut out = Vec::new();
    for _ in 0..20 {
        let at = r.next;
        r.send(at, RATE);
        out.extend(r.push(t0 + Duration::from_secs(1)));
    }
    r.push(t0 + Duration::from_secs(3));
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(inputs[0].anchor, Some(Anchor::SystemTime));
    assert_joined(&out[out.len() - 5..]);
}

/// Audio before this rig's first block is not anchored against an empty present (which
/// would put it a whole session index away): the frames after the first block anchor.
#[test]
fn audio_before_the_first_local_block_waits_to_anchor() {
    let mut r = Rig::new(false);
    r.next = 100_000;
    let t0 = Instant::now();
    r.send(r.next, RATE);
    let line = StampLine {
        epoch_stamp_ns: 0,
        system_ns: 0,
        ppm: 0.0,
    };
    assert!(r.joiner.push(&[], Some(line), t0).is_empty());
    let mut out = Vec::new();
    for j in 0..60u64 {
        let at = r.next;
        r.send(at, RATE);
        out.extend(r.push(t0 + Duration::from_millis(25 * j)));
    }
    r.push(t0 + Duration::from_secs(3));
    let (inputs, _) = r.last_report().expect("reported");
    assert_eq!(inputs[0].slips, 0);
    assert_joined(&out);
}

/// A first frame that waited in a queue is placed far back by system time although the
/// clocks agree: the fresh frames after it anchor on system time.
#[test]
fn a_stale_first_frame_is_no_clock_disagreement() {
    let mut r = Rig::new(true);
    r.next = 100_000;
    let t0 = Instant::now();
    for _ in 0..20 {
        r.push(t0);
    }
    // Captured half a second ago, it arrives only now.
    r.send(r.next - u64::from(RATE) / 2, RATE);
    r.push(t0);
    for _ in 0..10 {
        let at = r.next;
        r.send(at, RATE);
        r.push(t0 + Duration::from_millis(100));
    }
    r.push(t0 + Duration::from_secs(2));
    let (inputs, rigs) = r.last_report().expect("reported");
    assert_eq!(inputs[0].anchor, Some(Anchor::SystemTime));
    assert_eq!(rigs[0].disagree, None);
}
