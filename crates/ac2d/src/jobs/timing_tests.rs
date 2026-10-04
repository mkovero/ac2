//! The timing job on the simulated rig: without stimulus a hop reads only the newest window
//! of generator history and sends nothing nobody receives; once the generator runs, every
//! window measures exactly what a full-history window would, and the lock and the stop
//! reach control as before.

use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use ac2_audio::fake::FakePath;
use ac2_audio::generator::WhiteNoise;
use ac2_audio::{
    DuplexRequest, DuplexStream, FakeBackend, FakeConfig, FakeDriver, GeneratorHandle,
    HistoryRequest, MaxLevel, OutputSource, generator,
};
use ac2_core::timing::{LoopbackTiming, TimingConfig};
use ac2_proto::frame::FrameData;
use ac2_proto::model::{TimingState, TimingStatus};
use ac2_proto::topic::Topic;
use ac2_proto::units::{Samples, SessionEpoch};

use super::Timing;
use crate::control::ControlMsg;
use crate::conv;
use crate::fanout::pop_block;
use crate::jobs::pace_tests::Rig;
use crate::jobs::{Analysis, REFRESH};

const FS: u32 = 48_000;
const DELAY: u32 = 37;

struct Sim {
    stream: DuplexStream,
    driver: FakeDriver,
    generator: GeneratorHandle,
    job: Timing,
    control: Receiver<ControlMsg>,
    /// Loopback capture from sample 0, kept to replay windows against full history.
    capture: Vec<f32>,
    /// `(window, status)` after every window the job measured.
    windows: Vec<(ac2_proto::frame::TimingWindow, TimingStatus)>,
}

impl Sim {
    fn new() -> Self {
        let backend = FakeBackend::new(FakeConfig {
            paths: vec![FakePath::loopback(0, 0, DELAY)],
            ..FakeConfig::default()
        })
        .expect("config");
        let (mut generator, port) = generator([0]).expect("routes");
        generator
            .set_source(Box::new(WhiteNoise::new(0.1, 3)))
            .expect("queue");
        let mut req = DuplexRequest::new(vec![0], 1, MaxLevel::from_peak_db(-6.0).expect("level"));
        req.output = OutputSource::Generator(port);
        // Long enough to replay every window of a test against full history afterwards.
        req.history = Some(HistoryRequest {
            channel: 0,
            seconds: 30.0,
        });
        req.ring_seconds = 10.0;
        let (stream, driver) = backend.open_manual(req).expect("open");
        let history = stream.history().expect("history").clone();
        let (tx, control) = channel();
        let initial = TimingStatus {
            epoch: 0,
            state: TimingState::NoStimulus,
            last_lock: None,
            drift: None,
            internal_reference: false,
        };
        let job = Timing::new(FS, 0, history, tx, SessionEpoch(1), initial);
        Self {
            stream,
            driver,
            generator,
            job,
            control,
            capture: Vec::new(),
            windows: Vec::new(),
        }
    }

    /// Runs `seconds` of audio in hand-offs of four blocks, emitting after each.
    fn run(&mut self, seconds: f64, rig: &Rig) {
        let blocks = (seconds * f64::from(FS) / 256.0).round() as u64;
        for _ in 0..blocks.div_ceil(4) {
            self.driver.run_blocks(4);
            while let Some(b) = pop_block(&mut self.stream) {
                assert_eq!(b.start_sample, self.capture.len() as u64);
                self.capture.extend_from_slice(&b.data);
                let before = self.job.generation;
                self.job.push(&b);
                assert!(self.job.generation <= before + 1);
                if self.job.generation > before {
                    let w = self.job.last_window.expect("window");
                    self.windows.push((w, self.job.status));
                }
            }
            self.job.emit(&rig.em);
        }
    }

    fn committed(&self) -> Vec<TimingStatus> {
        self.control
            .try_iter()
            .filter_map(|m| match m {
                ControlMsg::Timing { status, .. } => Some(status),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn without_stimulus_a_hop_reads_no_full_history_and_sends_only_to_subscribers() {
    let rig = Rig::new("timing-quiet");
    let mut s = Sim::new();
    s.run(5.0, &rig);
    // About four hops a second once the first window is complete.
    assert!(s.windows.len() >= 15, "{} windows", s.windows.len());
    assert_eq!(s.job.full_reads, 0);
    assert!(
        s.windows
            .iter()
            .all(|(w, st)| st.state == TimingState::NoStimulus && w.offset.is_none())
    );
    assert!(rig.frames().is_empty(), "frames sent to nobody");
    // Between hops the job needs nothing from the hand-offs.
    let need = s.job.frames_needed().expect("idle between hops");
    assert!(need <= u64::from(FS) / 4, "{need}");

    // A subscriber gets each new window, and between windows only the refresh.
    rig.subscribe(Topic::Timing);
    let n = s.windows.len();
    let t0 = Instant::now();
    s.run(2.0, &rig);
    let new = s.windows.len() - n;
    let refreshes = (t0.elapsed().as_secs_f64() / REFRESH.as_secs_f64()).ceil() as usize;
    let frames = rig.frames();
    assert!(
        frames
            .iter()
            .all(|f| matches!(f.data, FrameData::Timing(_)))
    );
    assert!(
        frames.len() >= new && frames.len() <= new + refreshes,
        "{} frames for {new} windows",
        frames.len()
    );
    assert_eq!(s.job.full_reads, 0);
}

#[test]
fn with_stimulus_windows_match_full_history_and_lock_then_stop_reaches_control() {
    let rig = Rig::new("timing-stimulus");
    let mut s = Sim::new();
    s.run(2.0, &rig);
    let quiet = s.windows.len();
    s.generator.start();
    s.run(4.0, &rig);
    let locked = TimingState::Locked {
        offset: Samples(i64::from(DELAY)),
    };
    assert_eq!(s.job.status.state, locked);
    assert!(s.job.full_reads > 0);
    s.generator.stop();
    s.run(2.0, &rig);
    assert_eq!(s.job.status.state, TimingState::NoStimulus);
    assert_eq!(
        s.job.status.last_lock.map(|l| l.offset),
        Some(Samples(i64::from(DELAY)))
    );

    // Control saw the lock and then the stop, in that order.
    let states: Vec<_> = s.committed().iter().map(|c| c.state).collect();
    let lock_at = states.iter().position(|st| *st == locked).expect("lock");
    assert!(
        states[lock_at..].contains(&TimingState::NoStimulus),
        "{states:?}"
    );

    // Replaying every window against the whole history slice gives the same measurement
    // and state: the lock comes at the same window, at the same offset.
    let cfg = TimingConfig::for_rate(f64::from(FS));
    let mut full = LoopbackTiming::new(cfg);
    full.new_epoch();
    let history = s.stream.history().expect("history").clone();
    let mut reference = Vec::new();
    for (k, (w, st)) in s.windows.iter().enumerate() {
        let start = w.capture_start.0;
        let range = full.search_range();
        let r0 = range.reference_start(start);
        let len = range.reference_len(cfg.window);
        reference.clear();
        reference.resize(len, 0.0);
        let skip = usize::try_from(-r0.min(0)).unwrap_or(len).min(len);
        history
            .read(r0.max(0) as u64, &mut reference[skip..])
            .expect("history");
        let capture = &s.capture[start as usize..start as usize + cfg.window];
        let (m, _) = full
            .process_window(start, capture, &reference, range)
            .expect("window");
        assert_eq!(m.loopback_dbfs.to_bits(), w.loopback.0.to_bits(), "{k}");
        assert_eq!(m.stimulus_dbfs.to_bits(), w.stimulus.0.to_bits(), "{k}");
        let offset = match m.outcome {
            ac2_core::timing::Outcome::Offset(p) => Some(Samples(p.offset)),
            _ => None,
        };
        assert_eq!(offset, w.offset, "window {k}");
        assert_eq!(conv::timing_state(full.tracker().state()), st.state, "{k}");
    }
    assert!(s.windows.len() > quiet + 8);
}
