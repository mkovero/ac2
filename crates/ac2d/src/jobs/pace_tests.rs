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
        resolution: ac2_proto::model::SpecResolution::Fixed { fft_len: 4096 },
        window: Window::Hann,
        averaging: SpecAveraging::Off,
        smoothing: None,
        level: ac2_proto::model::SpecLevel::Tone,
        display: Default::default(),
    }
}

fn spectrum() -> Spectrum {
    spectrum_of(spectrum_config())
}

fn spectrum_of(cfg: SpectrumConfig) -> Spectrum {
    Spectrum::new(MeasId(1), cfg, FS, 0, InputCal::none(), Rev(1)).expect("spectrum")
}

/// A spectrum whose 8192-sample hop (32 blocks) leaves stretches of audio without a new
/// result; the first comes after 65 536 samples (256 blocks).
fn long_spectrum() -> Spectrum {
    spectrum_of(SpectrumConfig {
        resolution: ac2_proto::model::SpecResolution::Fixed { fft_len: 65_536 },
        ..spectrum_config()
    })
}

const LONG_FIRST: usize = 256;

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
            display: Default::default(),
        },
        FS,
        0,
        InputCal::none(),
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

    // Between two results of a long hop nothing changes while audio comes in; the result
    // is only re-sent at the refresh, so a client never sees a steady view go STALE.
    let r = Rig::new("unchanged-hop");
    r.subscribe(SPEC);
    let mut s = long_spectrum();
    let (mut at, mut seed) = (0, 12);
    feed(&mut s, &mut at, LONG_FIRST, 0.1, &mut seed);
    assert_eq!(s.emit(&r.em), Flush::Done);
    let old = r
        .frames()
        .into_iter()
        .find(|f| matches!(f.data, FrameData::Spec(_)))
        .expect("first spectrum");
    for _ in 0..10 {
        feed(&mut s, &mut at, 1, 0.1, &mut seed);
        // Newer audio: the refresh is pending.
        assert!(matches!(s.emit(&r.em), Flush::Pending(_)));
    }
    assert_eq!(r.count(Stream::Spec), 0);
    std::thread::sleep(REFRESH + Duration::from_millis(10));
    feed(&mut s, &mut at, 1, 0.1, &mut seed);
    assert_eq!(s.emit(&r.em), Flush::Done);
    let refreshed: Vec<Frame> = r
        .frames()
        .into_iter()
        .filter(|f| matches!(f.data, FrameData::Spec(_)))
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
    assert_eq!(r.count(Stream::Spec), 0);
}

/// The newest stamp still goes out after the last block, by the refresh at the latest, even
/// when that block brought no new result.
#[test]
fn the_last_block_is_stamped() {
    let r = Rig::new("last");
    r.subscribe(SPEC);
    let mut s = long_spectrum();
    let (mut at, mut seed) = (0, 13);
    feed(&mut s, &mut at, LONG_FIRST, 0.1, &mut seed);
    s.emit(&r.em);
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
        .rfind(|f| matches!(f.data, FrameData::Spec(_)))
        .expect("refresh");
    assert_eq!(last.stamp.audio_sample.0 + 1, at);
}

/// A 65 536-point spectrum at 96 kHz has an 8192-sample hop (32 blocks of 256), and audio
/// comes in hand-offs of 6 or 7 blocks (60 a second): the hop ends anywhere in a hand-off.
/// Each spectrum is still stamped where its frame ended, sample and wall clock, so frames
/// read as evenly spaced as they were analysed; the blocks after the frame do not shift it.
#[test]
fn spectra_are_stamped_where_their_frame_ended() {
    const FS96: u32 = 96_000;
    let r = Rig::new("hop-stamp");
    r.subscribe(SPEC);
    let mut s = Spectrum::new(
        MeasId(1),
        SpectrumConfig {
            resolution: ac2_proto::model::SpecResolution::Fixed { fft_len: 65_536 },
            ..spectrum_config()
        },
        FS96,
        0,
        InputCal::none(),
        Rev(1),
    )
    .expect("spectrum");
    let hop = u64::from(SpectrumConfig::display_hop(65_536, FS96));
    assert_eq!(hop, 8192);
    let wall0 = 1_000_000_000_000u64;
    let wall_of = |end: u64| wall0 + end * 1_000_000_000 / u64::from(FS96);
    let mut seed = 21;
    let mut at = 0u64;
    // 6.25 blocks per hand-off on average, as at 60 hand-offs a second.
    for k in 1..=200u64 {
        while at < k * 1600 / 256 * 256 {
            let mut b = block(at, 0.1, &mut seed);
            b.wall_ns = wall_of(at + u64::from(BLOCK));
            s.push(&b);
            at += u64::from(BLOCK);
        }
        s.emit(&r.em);
    }
    let spectra: Vec<Frame> = r
        .frames()
        .into_iter()
        .filter(|f| matches!(f.data, FrameData::Spec(_)))
        .collect();
    assert!(spectra.len() > 20, "{} spectra", spectra.len());
    for f in &spectra {
        let end = f.stamp.audio_sample.0 + 1;
        assert_eq!(end % hop, 0, "a spectrum stamped off its hop at {end}");
        let off_ns = f.stamp.capture_wall_ns.0.abs_diff(wall_of(end));
        assert!(off_ns < 1000, "wall clock {off_ns} ns off the frame's end");
    }
    for w in spectra.windows(2) {
        assert_eq!(w[1].stamp.audio_sample.0 - w[0].stamp.audio_sample.0, hop);
    }
}

/// A multi-resolution spectrum at 96 kHz: frames come at the shortest band's 1024-sample
/// hop, each stamped at a frame end; every band's age is exact from its own framing; the
/// columns only the longest window serves hold still between its spectra (every 8192
/// samples) while the top columns change with every frame, and no column has a value
/// before every band behind it has a spectrum.
#[test]
fn multires_bands_update_at_their_own_hops_with_exact_ages() {
    use ac2_core::multires::MultiResLadder;
    use ac2_proto::model::SpecResolution;
    const FS96: u32 = 96_000;
    let r = Rig::new("multires");
    r.subscribe(SPEC);
    let mut s = Spectrum::new(
        MeasId(1),
        SpectrumConfig {
            resolution: SpecResolution::MULTI,
            ..spectrum_config()
        },
        FS96,
        0,
        InputCal::none(),
        Rev(1),
    )
    .expect("spectrum");
    let ladder = super::spectrum::ladder(FS96).expect("ladder");
    let grid = MultiResLadder::grid(f64::from(FS96), 48);
    let mut seed = 5;
    let mut at = 0u64;
    let mut spectra = Vec::new();
    for k in 1..=160u64 {
        while at < k * 1600 / 256 * 256 {
            let mut b = block(at, 0.1, &mut seed);
            b.wall_ns = 1 + at;
            s.push(&b);
            at += u64::from(BLOCK);
        }
        s.emit(&r.em);
        spectra.extend(
            r.frames()
                .into_iter()
                .filter(|f| matches!(f.data, FrameData::Spec(_))),
        );
    }
    assert!(spectra.len() > 100, "{} spectra", spectra.len());
    let mut prev: Option<(u64, Vec<f32>)> = None;
    let (lf, hf): (Vec<usize>, Vec<usize>) = (
        (0..grid.len())
            .filter(|&c| grid.frequency(c) <= ladder.bands[0].anchor_hz)
            .collect(),
        (0..grid.len())
            .filter(|&c| grid.frequency(c) >= ladder.bands[5].anchor_hz)
            .collect(),
    );
    for f in &spectra {
        let end = f.stamp.audio_sample.0 + 1;
        assert_eq!(end % 1024, 0, "stamped off the top band's hop at {end}");
        let FrameData::Spec(spec) = &f.data else {
            unreachable!()
        };
        assert_eq!(spec.level.len(), grid.len());
        let bands = spec.meta.bands.as_ref().expect("bands");
        assert_eq!(bands.bands.len(), 6);
        for (band, meta) in ladder.bands.iter().zip(&bands.bands) {
            let (n, hop) = (band.n as u64, band.hop as u64);
            let want = (end >= n).then(|| (end - n) % hop);
            let got = meta.age.map(|a| (a.0 * f64::from(FS96)).round() as u64);
            assert_eq!(got, want, "band of {n} at {end}");
            assert!((meta.hop.0 * f64::from(FS96) - hop as f64).abs() < 1e-6);
        }
        if bands.bands[0].age.is_none() {
            assert!(
                lf.iter().all(|&c| spec.level[c].is_nan()),
                "LF before its window"
            );
        }
        if let Some((prev_end, p)) = &prev {
            let lf_same = lf
                .iter()
                .all(|&c| p[c].to_bits() == spec.level[c].to_bits());
            // A new longest-window spectrum since the previous frame (a hand-off can hold
            // two top-band hops, so it need not end exactly at this frame).
            let lf_new = bands.bands[0]
                .age
                .is_some_and(|a| end - (a.0 * f64::from(FS96)).round() as u64 > *prev_end);
            assert_eq!(lf_same, !lf_new, "LF columns at {end}");
            assert!(
                hf.iter().any(|&c| p[c] != spec.level[c]),
                "HF columns hold at {end}"
            );
        }
        prev = Some((end, spec.level.clone()));
    }
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
                .map(|c| c.frame)
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
            cue: None,
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
