//! A recorded file played back as a capture-only device, so the analyses that ran on the
//! live device run on the same samples again.
//!
//! The replay runs the shared plumbing (transport, header ring) like every backend; its
//! "device" is a thread that reads frames from a [`FrameSource`] and pushes them as blocks.
//! Unlike a real device it can wait: when the capture ring has no room it holds the next
//! block back instead of dropping it, so a replay never loses audio, however fast it runs.
//!
//! Block indices continue the recording's own: sample 0 is the file's first frame, and at
//! every recorded discontinuity ([`ReplayMark`]) the index jumps by the samples the
//! recording lost and the block carries the recorded condition bits, so the analyses reset
//! exactly where they reset when the audio was live. Blocks never straddle a mark.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::backend::{
    Backend, BackendKind, ClockRelation, Delivery, DeviceCaps, DeviceId, DeviceSelector, Direction,
    DirectionCaps, DuplexRequest, FrameRange, IndexExactness, Negotiated, RateRange, SampleFormat,
    StaticLatency,
};
use crate::block::{BlockFlags, BlockStamp};
use crate::error::{AudioError, Operation, Unsupported};
use crate::stream::{DuplexStream, Plumbing, StreamParts};

/// Where a replay reads its frames.
pub trait FrameSource: Send {
    /// Reads up to `out.len() / channels` interleaved frames of every recorded channel
    /// into `out`; returns the frames read, 0 at the end.
    fn read(&mut self, out: &mut [f32]) -> std::io::Result<usize>;
}

/// Opens a fresh [`FrameSource`] at the file's first frame; called on every stream open.
pub type OpenSource = Arc<dyn Fn() -> std::io::Result<Box<dyn FrameSource>> + Send + Sync>;

/// A recorded discontinuity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayMark {
    /// File frame of the first sample after it.
    pub frame: u64,
    /// Samples the recording lost before it.
    pub lost_frames: u64,
    /// What the block starting there carries (the configuration-change bit is never
    /// replayed: a replay has no device that could have changed, and the bit would make
    /// the daemon reopen the session).
    pub flags: BlockFlags,
}

/// How fast the replay device delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaySpeed {
    /// One block per block duration of wall-clock time.
    Realtime,
    /// As fast as the consumer takes blocks.
    Fast,
}

/// The recording a [`ReplayBackend`] plays.
#[derive(Clone)]
pub struct ReplayConfig {
    /// Device id the replay reports (the recorded device's).
    pub device_id: String,
    /// Name shown for the device.
    pub device_name: String,
    /// Rate.
    pub sample_rate: u32,
    /// Frames per block (the recorded device period).
    pub block_frames: u32,
    /// Device input of each file channel.
    pub inputs: Vec<u16>,
    /// Device input names, one per device input `0 ..= max(inputs)`.
    pub input_names: Vec<String>,
    /// Frames in the file.
    pub frames: u64,
    /// Recorded discontinuities, by frame.
    pub marks: Vec<ReplayMark>,
    /// Pace.
    pub speed: ReplaySpeed,
    /// The frames.
    pub open: OpenSource,
}

impl std::fmt::Debug for ReplayConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayConfig")
            .field("device_id", &self.device_id)
            .field("sample_rate", &self.sample_rate)
            .field("inputs", &self.inputs)
            .field("frames", &self.frames)
            .field("speed", &self.speed)
            .finish_non_exhaustive()
    }
}

/// A recording as a capture-only device.
#[derive(Debug, Clone)]
pub struct ReplayBackend {
    cfg: ReplayConfig,
    played: Arc<AtomicU64>,
    released: Arc<AtomicBool>,
}

impl ReplayBackend {
    /// A replay of `cfg`.
    pub fn new(cfg: ReplayConfig) -> Self {
        Self {
            cfg,
            played: Arc::new(AtomicU64::new(0)),
            released: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Lets the most recently opened stream play. A stream opens held: a file has no
    /// "before", so whatever reads the blocks attaches first and then releases it, and
    /// not one frame goes by unseen.
    pub fn release(&self) {
        self.released.store(true, Ordering::Release);
    }

    /// The recording.
    pub fn config(&self) -> &ReplayConfig {
        &self.cfg
    }

    /// File frames pushed by the most recent stream.
    pub fn frames_played(&self) -> u64 {
        self.played.load(Ordering::Acquire)
    }

    fn device_inputs(&self) -> u16 {
        self.cfg.inputs.iter().max().map_or(0, |m| m + 1)
    }
}

impl Backend for ReplayBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Replay
    }

    fn enumerate(&self) -> Result<Vec<DeviceCaps>, AudioError> {
        let c = &self.cfg;
        Ok(vec![DeviceCaps {
            backend: BackendKind::Replay,
            host: "replay".into(),
            id: DeviceId(c.device_id.clone()),
            name: c.device_name.clone(),
            input: Some(DirectionCaps {
                max_channels: self.device_inputs(),
                rates: vec![RateRange {
                    min: c.sample_rate,
                    max: c.sample_rate,
                }],
                buffer_frames: Some(FrameRange {
                    min: c.block_frames,
                    max: c.block_frames,
                }),
                formats: vec![SampleFormat::F32],
                default_rate: Some(c.sample_rate),
                default_buffer: Some(c.block_frames),
                channel_names: Some(c.input_names.clone()),
            }),
            output: None,
            duplex_clock: ClockRelation::SingleCallback,
            index: IndexExactness::Exact,
            latency: StaticLatency::Unknown,
            notes: vec!["a recording played back; capture only".into()],
        }])
    }

    fn open(&self, mut request: DuplexRequest) -> Result<DuplexStream, AudioError> {
        request.validate()?;
        let c = &self.cfg;
        if let DeviceSelector::Id(id) = &request.input_device
            && id.0 != c.device_id
        {
            return Err(AudioError::DeviceNotFound {
                direction: Direction::Input,
                selector: request.input_device.to_string(),
            });
        }
        if let Some(r) = request.sample_rate.filter(|&r| r != c.sample_rate) {
            return Err(Unsupported::SampleRate {
                requested: r,
                offered: format!("{} Hz only (the recorded rate)", c.sample_rate),
            }
            .into());
        }
        if let Some(b) = request.buffer_frames.filter(|&b| b != c.block_frames) {
            return Err(Unsupported::BufferFrames {
                requested: b,
                fixed: Some(c.block_frames),
            }
            .into());
        }
        if request.output_channels > 0 {
            return Err(Unsupported::OutputChannels {
                requested: request.output_channels,
                available: 0,
            }
            .into());
        }
        let mut columns = Vec::with_capacity(request.input_map.len());
        for &ch in &request.input_map {
            match c.inputs.iter().position(|&i| i == ch) {
                Some(col) => columns.push(col),
                None => {
                    return Err(Unsupported::InputNotRecorded { channel: ch }.into());
                }
            }
        }
        let block = c.block_frames.max(1);
        let plumbing = Plumbing::new(&mut request, c.sample_rate, block as usize);
        let negotiated = Negotiated {
            backend: BackendKind::Replay,
            input_device: DeviceId(c.device_id.clone()),
            output_device: DeviceId(c.device_id.clone()),
            sample_rate: c.sample_rate,
            input_channels: request.input_map.len() as u16,
            device_input_channels: self.device_inputs(),
            output_channels: 0,
            device_output_channels: 0,
            buffer_frames: Some(block),
            input_format: SampleFormat::F32,
            output_format: None,
            clock: ClockRelation::SingleCallback,
            index: IndexExactness::Exact,
            latency: StaticLatency::Unknown,
            delivery: Delivery::Stepped,
        };
        let source = (c.open)().map_err(|e| AudioError::Backend {
            backend: BackendKind::Replay,
            operation: Operation::Open,
            detail: e.to_string(),
        })?;
        let mut stream = DuplexStream::new(StreamParts {
            negotiated,
            capture: plumbing.consumer,
            output: plumbing.output,
            events: Arc::clone(&plumbing.events),
            drain: Duration::ZERO,
            guard: Box::new(()),
            patch: None,
        });
        self.played.store(0, Ordering::Release);
        self.released.store(false, Ordering::Release);
        let player = Player {
            producer: plumbing.producer,
            events: plumbing.events,
            source,
            columns,
            file_channels: c.inputs.len(),
            block,
            sample_rate: c.sample_rate,
            frames: c.frames,
            marks: c.marks.clone(),
            speed: c.speed,
            played: Arc::clone(&self.played),
            released: Arc::clone(&self.released),
        };
        let guard = player.spawn().map_err(|e| AudioError::Backend {
            backend: BackendKind::Replay,
            operation: Operation::Start,
            detail: e.to_string(),
        })?;
        stream.set_guard(Box::new(guard), Duration::ZERO);
        Ok(stream)
    }
}

struct Guard {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Player {
    producer: crate::block::BlockProducer,
    events: Arc<crate::events::BackendEvents>,
    source: Box<dyn FrameSource>,
    /// File channel of each stream channel.
    columns: Vec<usize>,
    file_channels: usize,
    block: u32,
    sample_rate: u32,
    frames: u64,
    marks: Vec<ReplayMark>,
    speed: ReplaySpeed,
    played: Arc<AtomicU64>,
    released: Arc<AtomicBool>,
}

/// How long the player waits for room in the capture ring before looking again.
const ROOM_POLL: Duration = Duration::from_millis(1);

impl Player {
    fn spawn(self) -> std::io::Result<Guard> {
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("ac2-replay".into())
            .spawn(move || self.run(&s))?;
        Ok(Guard {
            stop,
            thread: Some(thread),
        })
    }

    fn ns(&self, frames: u64) -> u64 {
        (u128::from(frames) * 1_000_000_000 / u128::from(self.sample_rate.max(1))) as u64
    }

    fn run(mut self, stop: &AtomicBool) {
        let (sleeper, _) = match crate::timer::sleeper() {
            Ok(s) => s,
            Err(_) => {
                self.events.error();
                return;
            }
        };
        while !self.released.load(Ordering::Acquire) {
            if stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(ROOM_POLL);
        }
        let mut buf = vec![0.0f32; self.block as usize * self.file_channels.max(1)];
        let mut frame = 0u64;
        let mut lost = 0u64;
        let mut next_mark = 0usize;
        let mut first = true;
        let t0 = Instant::now();
        let mut due = Duration::ZERO;
        while frame < self.frames && !stop.load(Ordering::Acquire) {
            let mut flags = if first {
                BlockFlags::FIRST
            } else {
                BlockFlags::NONE
            };
            while let Some(m) = self.marks.get(next_mark).filter(|m| m.frame <= frame) {
                lost += m.lost_frames;
                flags |= BlockFlags::from_bits_truncate(
                    m.flags.bits() & !BlockFlags::CONFIG_CHANGE.bits(),
                );
                if m.lost_frames > 0 {
                    flags |= BlockFlags::DISCONTINUITY;
                }
                next_mark += 1;
            }
            let until_mark = self
                .marks
                .get(next_mark)
                .map_or(u64::MAX, |m| m.frame - frame);
            let want = u64::from(self.block)
                .min(self.frames - frame)
                .min(until_mark);
            let want = want as usize;
            let got = match self.source.read(&mut buf[..want * self.file_channels]) {
                Ok(0) | Err(_) => {
                    // The file ended early or became unreadable: nothing more to play.
                    self.events.error();
                    break;
                }
                Ok(n) => n,
            };
            let frames = got as u32;
            while !self.producer.has_room(frames) {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(ROOM_POLL);
            }
            let start = frame + lost;
            let (cols, n, b) = (&self.columns, self.file_channels, &buf);
            self.producer.push_with(
                BlockStamp {
                    start_sample: start,
                    frames,
                    flags,
                    callback_ns: self.ns(start + u64::from(frames)),
                    capture_ns: Some(self.ns(start)),
                },
                |f, c| b[f * n + cols[c]],
            );
            first = false;
            frame += got as u64;
            self.played.store(frame, Ordering::Release);
            if self.speed == ReplaySpeed::Realtime {
                due += Duration::from_nanos(self.ns(u64::from(frames)));
                while Instant::now() < t0 + due && !stop.load(Ordering::Acquire) {
                    sleeper.sleep_until(t0 + due);
                }
            }
        }
        // The recording is over; the stream stays open, silent, until it is stopped.
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ramp {
        channels: usize,
        next: u64,
        frames: u64,
    }

    impl FrameSource for Ramp {
        fn read(&mut self, out: &mut [f32]) -> std::io::Result<usize> {
            let n = (out.len() / self.channels).min((self.frames - self.next) as usize);
            for f in 0..n {
                for c in 0..self.channels {
                    out[f * self.channels + c] = (self.next + f as u64) as f32 + c as f32 * 0.5;
                }
            }
            self.next += n as u64;
            Ok(n)
        }
    }

    fn backend(speed: ReplaySpeed, frames: u64, marks: Vec<ReplayMark>) -> ReplayBackend {
        ReplayBackend::new(ReplayConfig {
            device_id: "fake".into(),
            device_name: "replay".into(),
            sample_rate: 48_000,
            block_frames: 256,
            inputs: vec![1, 3],
            input_names: vec![
                "not recorded".into(),
                "Room".into(),
                "not recorded".into(),
                "Loop".into(),
            ],
            frames,
            marks,
            speed,
            open: Arc::new(move || {
                Ok(Box::new(Ramp {
                    channels: 2,
                    next: 0,
                    frames,
                }) as Box<dyn FrameSource>)
            }),
        })
    }

    fn request(map: Vec<u16>, outputs: u16) -> DuplexRequest {
        DuplexRequest::new(
            map,
            outputs,
            crate::MaxLevel::from_peak_db(-20.0).expect("level"),
        )
    }

    #[test]
    fn every_frame_arrives_once_in_order_with_marks_at_their_samples() {
        let marks = vec![ReplayMark {
            frame: 1000,
            lost_frames: 500,
            flags: BlockFlags::XRUN | BlockFlags::CONFIG_CHANGE,
        }];
        let b = backend(ReplaySpeed::Fast, 48_000, marks);
        let mut s = b.open(request(vec![3, 1], 0)).expect("open");
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            s.capture().pop_with(|h, _, _| *h).is_none(),
            "held until released"
        );
        b.release();
        let mut next_frame = 0u64;
        let mut seen_mark = false;
        let deadline = Instant::now() + Duration::from_secs(20);
        while next_frame < 48_000 && Instant::now() < deadline {
            let Some((h, data)) = s.capture().pop_with(|h, a, b| {
                let mut v = a.to_vec();
                v.extend_from_slice(b);
                (*h, v)
            }) else {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            };
            if next_frame == 0 {
                assert!(h.flags.contains(BlockFlags::FIRST));
            }
            let lost = if next_frame >= 1000 { 500 } else { 0 };
            assert_eq!(h.start_sample, next_frame + lost);
            if next_frame == 1000 {
                seen_mark = true;
                assert!(
                    h.flags
                        .contains(BlockFlags::XRUN | BlockFlags::DISCONTINUITY)
                );
                assert!(!h.flags.contains(BlockFlags::CONFIG_CHANGE));
            } else if next_frame > 0 {
                assert!(h.flags.is_empty(), "{:?} at {next_frame}", h.flags);
            }
            assert!(next_frame >= 1000 || next_frame + u64::from(h.frames) <= 1000);
            for f in 0..h.frames as usize {
                let file_frame = (next_frame + f as u64) as f32;
                // Stream channel 0 is input 3 = file channel 1.
                assert_eq!(data[f * 2], file_frame + 0.5);
                assert_eq!(data[f * 2 + 1], file_frame);
            }
            next_frame += u64::from(h.frames);
        }
        assert_eq!(next_frame, 48_000);
        assert!(seen_mark);
        assert_eq!(
            s.transport_stats().blocks_dropped,
            0,
            "a replay never drops"
        );
        assert_eq!(b.frames_played(), 48_000);
    }

    #[test]
    fn unrecorded_inputs_outputs_and_other_rates_are_refused() {
        let b = backend(ReplaySpeed::Realtime, 1000, Vec::new());
        assert!(matches!(
            b.open(request(vec![0], 0)),
            Err(AudioError::Unsupported(Unsupported::InputNotRecorded {
                channel: 0
            }))
        ));
        assert!(matches!(
            b.open(request(vec![1], 1)),
            Err(AudioError::Unsupported(Unsupported::OutputChannels { .. }))
        ));
        let mut r = request(vec![1], 0);
        r.sample_rate = Some(44_100);
        assert!(b.open(r).is_err());
        let caps = b.enumerate().expect("caps");
        assert_eq!(caps[0].input.as_ref().map(|i| i.max_channels), Some(4));
        assert!(caps[0].output.is_none());
    }
}
