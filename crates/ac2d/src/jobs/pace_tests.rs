//! What a job sends, and when: nothing nobody receives, nothing unchanged except the
//! refresh that keeps a steady result fresh, captures whether or not anyone receives, and
//! every SPL peak at the reduced SPL frame rate.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac2_audio::BlockFlags;
use ac2_proto::frame::{Frame, FrameData};
use ac2_proto::model::{
    BandFraction, LeqConfig, PeakWeighting, RtaConfig, SpecAveraging, SpectrumConfig, SplConfig,
    TimeWeighting, Weighting, Window,
};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{DaemonIncarnation, Hz, MeasId, Rev, SessionEpoch};
use ac2_zmq::{Context, Socket, SocketType};

use super::rta::Rta;
use super::spectrum::Spectrum;
use super::spl::{LeqSetup, SPL_FPS, Spl};
use super::{Analysis, Emitter, Flush, JobCmd, JobEnv, JobMsg, REFRESH, Seqs, StampArgs};
use crate::calstore::InputCal;
use crate::fanout::Block;
use crate::io::Interest;

const FS: u32 = 48_000;
const BLOCK: u32 = 256;

pub(super) struct Rig {
    pull: Socket,
    env: JobEnv,
    pub(super) em: Emitter,
    _ctx: Context,
}

impl Rig {
    pub(super) fn new(name: &str) -> Self {
        let ctx = Context::new().expect("context");
        let endpoint = format!("inproc://pace-{name}");
        let pull = ctx.socket(SocketType::Pull).expect("pull");
        pull.bind(&endpoint).expect("bind");
        let env = JobEnv {
            ctx: ctx.clone(),
            endpoint,
            incarnation: DaemonIncarnation(1),
            epoch: SessionEpoch(1),
            seqs: Arc::new(Seqs::default()),
            interest: Arc::new(Interest::default()),
            fps: 60,
        };
        let em = Emitter::connect(env.clone()).expect("emitter");
        Self {
            pull,
            env,
            em,
            _ctx: ctx,
        }
    }

    pub(super) fn subscribe(&self, t: Topic) {
        self.env.interest.subscribe(&t.to_bytes());
    }

    fn unsubscribe(&self, t: Topic) {
        self.env.interest.unsubscribe(&t.to_bytes());
    }

    /// Frames that reached the I/O end since the last call.
    pub(super) fn frames(&self) -> Vec<Frame> {
        let mut out = Vec::new();
        while let Some(m) = self
            .pull
            .recv_timeout(Duration::from_millis(20))
            .expect("recv")
        {
            let parts: Vec<&[u8]> = m.frames().iter().map(Vec::as_slice).collect();
            out.push(ac2_proto::decode_frame(&parts[1..]).expect("frame"));
        }
        out
    }

    fn count(&self, stream: Stream) -> usize {
        self.frames()
            .iter()
            .filter(|f| matches!(f.data.topic(), Topic::Data { stream: s, .. } if s == stream))
            .count()
    }
}

/// White-ish noise at `amp` (deterministic).
fn block(start: u64, amp: f32, seed: &mut u64) -> Block {
    let data: Vec<f32> = (0..BLOCK)
        .map(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            ((*seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 2.0 * amp
        })
        .collect();
    Block::new(start, BLOCK, 1, BlockFlags::NONE, 1, data)
}

const SPEC: Topic = Topic::Data {
    meas: MeasId(1),
    stream: Stream::Spec,
};

fn spectrum_config() -> SpectrumConfig {
    SpectrumConfig {
        input: 0,
        fft_len: 4096,
        window: Window::Hann,
        averaging: SpecAveraging::Off,
        smoothing: None,
    }
}

fn spectrum() -> Spectrum {
    Spectrum::new(
        MeasId(1),
        spectrum_config(),
        FS,
        0,
        InputCal::none(),
        false,
        Rev(1),
    )
    .expect("spectrum")
}

/// Pushes `n` blocks from `*at`.
fn feed(a: &mut dyn Analysis, at: &mut u64, n: usize, amp: f32, seed: &mut u64) {
    for _ in 0..n {
        a.push(&block(*at, amp, seed));
        *at += u64::from(BLOCK);
    }
}

#[test]
fn nothing_is_built_or_sent_for_nobody() {
    let r = Rig::new("unsubscribed");
    let mut s = spectrum();
    let (mut at, mut seed) = (0, 7);
    for _ in 0..40 {
        feed(&mut s, &mut at, 8, 0.1, &mut seed);
        s.emit(&r.em);
    }
    // Neither the spectrum nor the input meters.
    assert!(r.frames().is_empty());
    // A subscriber gets the result on the next emit, and stops getting it when it leaves.
    r.subscribe(SPEC);
    feed(&mut s, &mut at, 8, 0.1, &mut seed);
    s.emit(&r.em);
    assert_eq!(r.count(Stream::Spec), 1);
    r.unsubscribe(SPEC);
    feed(&mut s, &mut at, 8, 0.1, &mut seed);
    s.emit(&r.em);
    assert_eq!(r.count(Stream::Spec), 0);
}

const RTA: Topic = Topic::Data {
    meas: MeasId(1),
    stream: Stream::Rta,
};

fn rta() -> Rta {
    Rta::new(
        MeasId(1),
        RtaConfig {
            input: 0,
            fraction: BandFraction::Third,
            f_lo: Hz(25.0),
            f_hi: Hz(16_000.0),
            weighting: Weighting::Z,
            averaging: SpecAveraging::Off,
        },
        FS,
        0,
        InputCal::none(),
        false,
        Rev(1),
    )
    .expect("rta")
}

#[test]
fn an_unchanged_result_is_only_refreshed() {
    let r = Rig::new("unchanged");
    r.subscribe(RTA);
    let mut s = rta();
    let (mut at, mut seed) = (0, 11);
    // Live: every interval with audio is a new result.
    for _ in 0..3 {
        feed(&mut s, &mut at, 4, 0.1, &mut seed);
        assert_eq!(s.emit(&r.em), Flush::Done);
    }
    assert_eq!(r.count(Stream::Rta), 3);
    // An emit without audio has nothing new: nothing goes out.
    assert_eq!(s.emit(&r.em), Flush::Done);
    assert_eq!(r.count(Stream::Rta), 0);

    // Frozen: the command changes the result once, then nothing changes; it is only
    // re-sent at the refresh, so a client never sees a frozen view go STALE.
    s.command(JobCmd::Freeze(true));
    feed(&mut s, &mut at, 4, 0.1, &mut seed);
    s.emit(&r.em);
    let frozen = r.frames();
    let old = frozen
        .iter()
        .find(|f| matches!(f.data, FrameData::Rta(_)))
        .expect("frozen rta");
    for _ in 0..10 {
        feed(&mut s, &mut at, 4, 0.1, &mut seed);
        // Newer audio: the refresh is pending.
        assert!(matches!(s.emit(&r.em), Flush::Pending(_)));
    }
    assert_eq!(r.count(Stream::Rta), 0);
    std::thread::sleep(REFRESH + Duration::from_millis(10));
    feed(&mut s, &mut at, 1, 0.1, &mut seed);
    assert_eq!(s.emit(&r.em), Flush::Done);
    let refreshed: Vec<Frame> = r
        .frames()
        .into_iter()
        .filter(|f| matches!(f.data, FrameData::Rta(_)))
        .collect();
    assert_eq!(refreshed.len(), 1);
    // Same result, newer stamp.
    assert_eq!(refreshed[0].data, old.data);
    assert_eq!(refreshed[0].stamp.audio_sample.0 + 1, at);
    assert!(refreshed[0].stamp.seq > old.stamp.seq);

    // Audio stops: nothing more goes out, so the client's STALE tells the truth.
    assert_eq!(s.emit(&r.em), Flush::Done);
    std::thread::sleep(REFRESH + Duration::from_millis(10));
    assert_eq!(s.emit(&r.em), Flush::Done);
    assert_eq!(r.count(Stream::Rta), 0);
}

/// The newest stamp still goes out after the last block, by the refresh at the latest, even
/// when that block brought no new result.
#[test]
fn the_last_block_is_stamped() {
    let r = Rig::new("last");
    r.subscribe(RTA);
    let mut s = rta();
    let (mut at, mut seed) = (0, 13);
    feed(&mut s, &mut at, 4, 0.1, &mut seed);
    s.emit(&r.em);
    s.command(JobCmd::Freeze(true));
    feed(&mut s, &mut at, 4, 0.1, &mut seed);
    s.emit(&r.em);
    feed(&mut s, &mut at, 1, 0.1, &mut seed);
    assert!(matches!(s.emit(&r.em), Flush::Pending(_)));
    let t0 = std::time::Instant::now();
    while matches!(s.emit(&r.em), Flush::Pending(_)) {
        assert!(t0.elapsed() < 2 * REFRESH);
        std::thread::sleep(Duration::from_millis(10));
    }
    let last = r
        .frames()
        .into_iter()
        .rfind(|f| matches!(f.data, FrameData::Rta(_)))
        .expect("refresh");
    assert_eq!(last.stamp.audio_sample.0 + 1, at);
}

#[test]
fn capture_works_with_nobody_subscribed() {
    let r = Rig::new("capture");
    let (h, feed) = super::spawn(
        "pace-capture".into(),
        r.env.clone(),
        1,
        Box::new(spectrum()),
    )
    .expect("spawn");
    // Nothing yet to capture.
    assert!(h.capture().is_none());
    let mut seed = 3;
    let batch: Vec<Block> = (0..32)
        .map(|i| block(i * u64::from(BLOCK), 0.1, &mut seed))
        .collect();
    feed.tx
        .send(super::JobMsg::Blocks(Arc::new(batch)))
        .expect("feed");
    // The job answers once it has taken the blocks; give it a moment to do so.
    let f = (0..50)
        .find_map(|_| {
            let f = h
                .capture()
                .filter(|f| f.stamp.audio_sample.0 + 1 == 32 * 256);
            if f.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
            f
        })
        .expect("capture");
    let FrameData::Spec(spec) = &f.data else {
        panic!("{:?}", f.data.topic());
    };
    assert_eq!(spec.level.len(), 4096 / 2 + 1);
    assert!(spec.level[100].is_finite());
    assert_eq!(f.stamp.session_epoch, SessionEpoch(1));
    // Every bin, on the FFT's own grid, not the live display columns.
    assert_eq!(
        f.stamp.grid_id,
        Some(super::spectrum::capture_grid(&spectrum_config(), FS).id())
    );
    // No spectrum was published to get there.
    assert_eq!(r.count(Stream::Spec), 0);
}

fn spl() -> Spl {
    let (to_control, _) = std::sync::mpsc::channel();
    Spl::new(
        MeasId(1),
        SplConfig {
            bands: None,
            input: 0,
            weighting: Weighting::Z,
            time_weighting: TimeWeighting::Fast,
            peak_weighting: PeakWeighting::Z,
            leq: LeqConfig::default_windows(),
            position: None,
        },
        FS,
        0,
        InputCal::none(),
        false,
        Rev(1),
        LeqSetup {
            log: Arc::new(Mutex::new(crate::leq_log::LeqLog::default())),
            to_control,
            judgements: Vec::new(),
            peak_judgements: [ac2_proto::model::LeqJudgement::NoLimit; 2],
            local: crate::config::LocalClock::Host,
        },
    )
    .expect("spl")
}

const SPL: Topic = Topic::Data {
    meas: MeasId(1),
    stream: Stream::Spl,
};

fn spl_frames(r: &Rig) -> Vec<ac2_proto::frame::SplMeta> {
    r.frames()
        .into_iter()
        .filter_map(|f| match f.data {
            FrameData::Spl(s) => Some(s.meta),
            _ => None,
        })
        .collect()
}

/// At `SPL_FPS` a one-block burst between two frames still shows in the next frame's Lpeak
/// and Lmax: they cover the meter's interval, not the time between frames.
#[test]
fn spl_peaks_survive_the_frame_rate() {
    let r = Rig::new("spl");
    r.subscribe(SPL);
    let mut s = spl();
    let (mut at, mut seed) = (0, 5);
    feed(&mut s, &mut at, 40, 0.01, &mut seed);
    s.emit(&r.em);
    let first = spl_frames(&r);
    assert_eq!(first.len(), 1);
    // A burst 30 dB up, emitted at once. Within the frame period nothing goes out yet and the
    // emit asks to be called again; a slow host may already be past the period, and then the
    // frame goes out now (the cap itself is `spl_frame_rate_is_capped`).
    feed(&mut s, &mut at, 1, 0.316, &mut seed);
    match s.emit(&r.em) {
        Flush::Pending(_) => assert!(spl_frames(&r).is_empty()),
        Flush::Done => {}
    }
    // Quiet again for half a second, then the next frame: it still holds the burst.
    feed(&mut s, &mut at, 100, 0.01, &mut seed);
    std::thread::sleep(Duration::from_secs_f64(1.0 / f64::from(SPL_FPS)));
    assert_eq!(s.emit(&r.em), Flush::Done);
    let next = spl_frames(&r);
    assert!(!next.is_empty());
    let (a, b) = (&first[0], next.last().expect("a frame after the quiet"));
    assert!(b.lpeak > a.lpeak + 25.0, "Lpeak {} → {}", a.lpeak, b.lpeak);
    assert!(b.lmax > a.lmax + 10.0, "Lmax {} → {}", a.lmax, b.lmax);
    // The running level has fallen back by then: only the interval figures held the burst.
    assert!(b.level < b.lmax - 10.0, "level {} Lmax {}", b.level, b.lmax);
}

/// The SPL topic goes out at most `SPL_FPS` times a second however often the job emits.
#[test]
fn spl_frame_rate_is_capped() {
    let r = Rig::new("spl-rate");
    r.subscribe(SPL);
    let mut s = spl();
    let (mut at, mut seed) = (0, 9);
    let t0 = std::time::Instant::now();
    while t0.elapsed() < Duration::from_millis(500) {
        feed(&mut s, &mut at, 1, 0.01, &mut seed);
        s.emit(&r.em);
        std::thread::sleep(Duration::from_millis(5));
    }
    let n = spl_frames(&r).len();
    let max = (0.5 * f64::from(SPL_FPS)).ceil() as usize + 1;
    assert!((5..=max).contains(&n), "{n} frames in 0.5 s");
}

/// A job that has nothing to do until much more audio has arrived.
struct Sleepy;

impl Analysis for Sleepy {
    fn result_generation(&self) -> Option<u64> {
        None
    }

    fn push(&mut self, _b: &Block) {}
    fn command(&mut self, _c: JobCmd) {}
    fn emit(&mut self, _e: &Emitter) -> Flush {
        Flush::Done
    }
    fn capture(&mut self) -> Option<(StampArgs, FrameData)> {
        None
    }
    fn frames_needed(&self) -> Option<u64> {
        Some(u64::MAX)
    }
}

/// A job sleeping through hand-offs still answers a capture and stops at once.
#[test]
fn an_idle_job_wakes_for_anything_but_audio() {
    let r = Rig::new("idle");
    let (h, feed) = super::spawn("idle".into(), r.env.clone(), 1, Box::new(Sleepy)).expect("job");
    let block = Block::new(0, BLOCK, 1, BlockFlags::NONE, 0, vec![0.0; BLOCK as usize]);
    feed.queue
        .frames
        .fetch_add(u64::from(BLOCK), Ordering::AcqRel);
    feed.tx
        .send(JobMsg::Blocks(Arc::new(vec![block])))
        .expect("send");
    // The job takes the batch, then parks for its idle time.
    let t = Instant::now();
    while feed.queue.frames.load(Ordering::Acquire) > 0 {
        assert!(t.elapsed() < Duration::from_secs(5), "batch never taken");
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(50));
    let t = Instant::now();
    assert!(h.capture().is_none());
    drop(h);
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}
