//! End-to-end tests through the shared plumbing with the simulated device. No hardware,
//! no wall-clock dependence (manual drive) except where a test says so.

use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakeFault, FakePath, InputContent, Pace, signature};
use ac2_audio::generator::WhiteNoise;
use ac2_audio::{
    AudioError, Backend, BlockFlags, BlockHeader, DeviceId, DeviceSelector, DuplexRequest,
    DuplexStream, FakeBackend, FakeConfig, FakeDriver, GeneratorHandle, HistoryRequest, MaxLevel,
    OutputSource, OutputState, StopOutcome, Unsupported, generator,
};

const RATE: u32 = 48_000;
const BLOCK: u32 = 256;

fn max_level() -> MaxLevel {
    MaxLevel::from_peak_db(-6.0).expect("level")
}

/// Drains every queued block.
fn drain(stream: &mut DuplexStream) -> Vec<(BlockHeader, Vec<f32>)> {
    let mut out = Vec::new();
    let mut buf = Vec::new();
    while let Some(h) = stream.capture().pop_into(&mut buf) {
        out.push((h, buf.clone()));
    }
    out
}

/// Channel `ch` of all blocks, concatenated, with the index of the first frame.
fn channel(blocks: &[(BlockHeader, Vec<f32>)], ch: usize) -> (u64, Vec<f32>) {
    let start = blocks.first().map_or(0, |(h, _)| h.start_sample);
    let mut v = Vec::new();
    for (h, data) in blocks {
        let n = usize::from(h.channels);
        v.extend((0..h.frames as usize).map(|f| data[f * n + ch]));
    }
    (start, v)
}

/// A fake with a white-noise generator on output 0, history of output 0, capture of
/// inputs 0 and 1, driven manually.
fn noise_rig(cfg: FakeConfig) -> (DuplexStream, FakeDriver, GeneratorHandle) {
    let backend = FakeBackend::new(cfg).expect("config");
    let (mut gen_handle, port) = generator([0]).expect("routes");
    gen_handle
        .set_source(Box::new(WhiteNoise::new(0.1, 99)))
        .expect("queue");
    gen_handle.start();
    let mut req = DuplexRequest::new(vec![0, 1], 2, max_level());
    req.output = OutputSource::Generator(port);
    req.history = Some(HistoryRequest::channel(0));
    req.ring_seconds = 10.0;
    let (stream, driver) = backend.open_manual(req).expect("open");
    (stream, driver, gen_handle)
}

/// The history samples for output indices `start..start + len`.
fn hist(stream: &DuplexStream, start: u64, len: usize) -> Vec<f32> {
    let mut v = vec![0.0; len];
    stream
        .history()
        .expect("history")
        .read(start, &mut v)
        .expect("history range");
    v
}

/// Integer lag `l` in `range` maximising Σ cap[n]·out[n − l] over `n` in `at..at + len`
/// (capture and output indices coincide on the fake).
fn best_lag(
    stream: &DuplexStream,
    cap: &[f32],
    cap_start: u64,
    at: u64,
    len: usize,
    range: std::ops::RangeInclusive<i64>,
) -> i64 {
    let c = &cap[(at - cap_start) as usize..][..len];
    let mut best = (f64::MIN, 0);
    for lag in range {
        let o = hist(stream, (at as i64 - lag) as u64, len);
        let score: f64 = c
            .iter()
            .zip(&o)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum();
        if score > best.0 {
            best = (score, lag);
        }
    }
    best.1
}

#[test]
fn routes_selected_channels_with_contiguous_exact_indices() {
    let backend = FakeBackend::new(FakeConfig {
        input_content: InputContent::Signature,
        counter_origin: u32::MAX - 1000,
        ..FakeConfig::default()
    })
    .expect("config");
    let (mut s, mut d) = backend
        .open_manual(DuplexRequest::new(vec![3, 1], 2, max_level()))
        .expect("open");
    d.run_blocks(50);
    let blocks = drain(&mut s);
    assert_eq!(blocks.len(), 50);
    for (i, (h, data)) in blocks.iter().enumerate() {
        assert_eq!(
            h.start_sample,
            i as u64 * u64::from(BLOCK),
            "contiguous across wrap"
        );
        assert_eq!(h.channels, 2);
        let expect = if i == 0 {
            BlockFlags::FIRST
        } else {
            BlockFlags::NONE
        };
        assert_eq!(h.flags, expect);
        for f in 0..h.frames as usize {
            let idx = h.start_sample + f as u64;
            assert_eq!(data[f * 2], signature(3, idx));
            assert_eq!(data[f * 2 + 1], signature(1, idx));
        }
    }
    // Simulated device time: capture time of block i is its first frame's time.
    let (h, _) = &blocks[10];
    assert_eq!(h.capture_ns, Some(2560 * 1_000_000_000 / u64::from(RATE)));
    // Output ticks share the capture index on a single-callback device.
    let t = s.pop_output_tick().expect("tick");
    assert_eq!(
        (t.start_sample, t.frames, t.state),
        (0, BLOCK, OutputState::Silent)
    );
}

#[test]
fn loopback_hears_generator_history_at_exact_delay() {
    for delay in [0u32, 37, 4800] {
        let (mut s, mut d, _g) = noise_rig(FakeConfig {
            paths: vec![FakePath::loopback(0, 1, delay)],
            ..FakeConfig::default()
        });
        d.run_seconds(0.5);
        let blocks = drain(&mut s);
        let (start, cap) = channel(&blocks, 1);
        let from = u64::from(delay) + 2000;
        let out = hist(&s, from - u64::from(delay), 10_000);
        assert_eq!(
            &cap[(from - start) as usize..][..10_000],
            &out[..],
            "delay {delay}"
        );
        // Input 0 has no path: silence.
        assert!(channel(&blocks, 0).1.iter().all(|&v| v == 0.0));
    }
}

#[test]
fn acoustic_path_applies_fir_delay_and_noise() {
    let fir = vec![0.5, -0.25, 0.125];
    let (mut s, mut d, _g) = noise_rig(FakeConfig {
        paths: vec![FakePath::acoustic(0, 0, 100, fir.clone(), 0.001)],
        ..FakeConfig::default()
    });
    d.run_seconds(0.5);
    let blocks = drain(&mut s);
    let (start, cap) = channel(&blocks, 0);
    let from = 5000u64;
    let len = 10_000;
    let out = hist(&s, from - 100 - 2, len + 2);
    let mut err2 = 0.0f64;
    for i in 0..len {
        let model: f32 = (0..3).map(|t| fir[t] * out[i + 2 - t]).sum();
        let e = f64::from(cap[(from - start) as usize + i] - model);
        err2 += e * e;
    }
    let rms = (err2 / len as f64).sqrt();
    assert!((rms - 0.001).abs() < 0.0001, "residual rms {rms}");
}

#[test]
fn dropped_output_frames_shift_offset_by_minus_k_without_flags() {
    let delay = 500;
    let (mut s, mut d, _g) = noise_rig(FakeConfig {
        paths: vec![FakePath::loopback(0, 1, delay)],
        faults: vec![FakeFault::DropOutputFrames {
            at_output_sample: 10_000,
            frames: 17,
        }],
        ..FakeConfig::default()
    });
    d.run_seconds(1.0);
    let blocks = drain(&mut s);
    assert!(blocks.iter().skip(1).all(|(h, _)| h.flags.is_empty()));
    let (start, cap) = channel(&blocks, 1);
    let before = best_lag(&s, &cap, start, 5000, 2048, 400..=600);
    let after = best_lag(&s, &cap, start, 20_000, 2048, 400..=600);
    assert_eq!((before, after), (500, 483));
    assert_eq!(d.stats().output_frames_dropped, 17);
    assert_eq!(d.stats().dac_underreads, 0);
}

#[test]
fn repeated_output_frames_shift_offset_by_plus_k() {
    let (mut s, mut d, _g) = noise_rig(FakeConfig {
        paths: vec![FakePath::loopback(0, 1, 500)],
        faults: vec![FakeFault::RepeatOutputFrames {
            at_output_sample: 10_000,
            frames: 64,
        }],
        ..FakeConfig::default()
    });
    d.run_seconds(1.0);
    let blocks = drain(&mut s);
    let (start, cap) = channel(&blocks, 1);
    let after = best_lag(&s, &cap, start, 20_000, 2048, 400..=700);
    assert_eq!(after, 564);
    assert_eq!(d.stats().output_frames_repeated, 64);
}

#[test]
fn clock_drift_moves_offset_at_the_configured_rate() {
    // 1000 ppm: the offset shrinks by 48 samples per second at 48 kHz.
    let (mut s, mut d, _g) = noise_rig(FakeConfig {
        paths: vec![FakePath::loopback(0, 1, 200)],
        drift_ppm: 1000.0,
        ..FakeConfig::default()
    });
    d.run_seconds(1.2);
    let blocks = drain(&mut s);
    let (start, cap) = channel(&blocks, 1);
    let early = best_lag(&s, &cap, start, 1000, 2048, 100..=250);
    let late = best_lag(&s, &cap, start, 48_000, 2048, 100..=250);
    assert!((early - 199).abs() <= 1, "early {early}");
    // Mid-window index ≈ 49 024 → expected offset 200 − 49.0.
    assert!((late - 151).abs() <= 1, "late {late}");
    assert_eq!(d.stats().dac_underreads, 0);
}

#[test]
fn xrun_flag_without_counter_jump_keeps_indices_and_offset() {
    let (mut s, mut d, _g) = noise_rig(FakeConfig {
        paths: vec![FakePath::loopback(0, 1, 300)],
        faults: vec![FakeFault::XrunFlag { at_block: 20 }],
        ..FakeConfig::default()
    });
    d.run_blocks(60);
    let blocks = drain(&mut s);
    for (i, (h, _)) in blocks.iter().enumerate() {
        assert_eq!(h.start_sample, i as u64 * u64::from(BLOCK));
        assert_eq!(h.flags.contains(BlockFlags::XRUN), i == 20);
        assert_eq!(h.flags.breaks_continuity(), i == 20);
    }
    let (start, cap) = channel(&blocks, 1);
    assert_eq!(best_lag(&s, &cap, start, 8000, 2048, 200..=400), 300);
    assert_eq!(s.events().xruns, 1);
}

#[test]
fn xrun_with_lost_frames_jumps_index_exactly_and_keeps_offset() {
    let (mut s, mut d, _g) = noise_rig(FakeConfig {
        paths: vec![FakePath::loopback(0, 1, 300)],
        faults: vec![
            FakeFault::Xrun {
                at_block: 20,
                lost_frames: 512,
            },
            FakeFault::ConfigChange { at_block: 40 },
        ],
        ..FakeConfig::default()
    });
    d.run_blocks(80);
    let blocks = drain(&mut s);
    let (h, _) = &blocks[20];
    assert_eq!(h.start_sample, 20 * 256 + 512);
    assert!(
        h.flags
            .contains(BlockFlags::XRUN | BlockFlags::DISCONTINUITY)
    );
    assert!(
        !h.flags.contains(BlockFlags::GAP_ESTIMATED),
        "exact counter"
    );
    assert_eq!(blocks[40].0.flags, BlockFlags::CONFIG_CHANGE);
    let t = (0..21)
        .filter_map(|_| s.pop_output_tick())
        .last()
        .expect("ticks");
    assert_eq!(
        t.start_sample, h.start_sample,
        "output shares the capture index"
    );
    assert!(t.flags.contains(BlockFlags::XRUN));
    // Capture after the gap still hears output at the same offset.
    let after: Vec<_> = blocks[21..].to_vec();
    let (start, cap) = channel(&after, 1);
    assert_eq!(
        best_lag(&s, &cap, start, start + 2000, 2048, 200..=400),
        300
    );
}

#[test]
fn slow_consumer_overflow_is_flagged_and_counted() {
    let backend = FakeBackend::new(FakeConfig::default()).expect("config");
    let mut req = DuplexRequest::new(vec![0], 1, max_level());
    req.ring_seconds = 0.05; // 2400 frames: 9 blocks of 256.
    let (mut s, mut d) = backend.open_manual(req).expect("open");
    d.run_blocks(20);
    let first = drain(&mut s);
    assert_eq!(first.len(), 9);
    d.step();
    let next = drain(&mut s);
    assert_eq!(next[0].0.start_sample, 20 * 256);
    assert!(
        next[0]
            .0
            .flags
            .contains(BlockFlags::OVERFLOW | BlockFlags::DISCONTINUITY)
    );
    let st = s.transport_stats();
    assert_eq!((st.blocks_dropped, st.frames_dropped), (11, 11 * 256));
}

#[test]
fn input_noise_has_configured_rms_and_runs_are_deterministic() {
    let run = || {
        let backend = FakeBackend::new(FakeConfig {
            input_noise_rms: 0.01,
            seed: 5,
            ..FakeConfig::default()
        })
        .expect("config");
        let (mut s, mut d) = backend
            .open_manual(DuplexRequest::new(vec![0, 2], 1, max_level()))
            .expect("open");
        d.run_blocks(100);
        channel(&drain(&mut s), 1).1
    };
    let a = run();
    assert_eq!(a, run());
    let rms = (a.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / a.len() as f64).sqrt();
    assert!((rms - 0.01).abs() < 0.0005, "{rms}");
}

#[test]
fn max_level_holds_end_to_end() {
    let backend = FakeBackend::new(FakeConfig {
        paths: vec![FakePath::loopback(0, 0, 10)],
        ..FakeConfig::default()
    })
    .expect("config");
    let (mut g, port) = generator([0]).expect("routes");
    g.set_source(Box::new(WhiteNoise::new(1.0, 3)))
        .expect("queue");
    g.start();
    let mut req = DuplexRequest::new(vec![0], 1, MaxLevel::from_peak_db(-20.0).expect("level"));
    req.output = OutputSource::Generator(port);
    let (mut s, mut d) = backend.open_manual(req).expect("open");
    d.run_blocks(40);
    let cap = channel(&drain(&mut s), 0).1;
    assert!(cap.iter().all(|v| v.abs() <= 0.1 + 1e-7));
    assert!(cap.iter().any(|v| v.abs() > 0.099));
    assert!(s.output_stats().limited_samples > 0);
}

#[test]
fn stop_fades_out_over_twenty_ms_then_silence() {
    let (mut s, mut d, g) = noise_rig(FakeConfig {
        paths: vec![FakePath::loopback(0, 1, 0)],
        ..FakeConfig::default()
    });
    d.run_blocks(20);
    assert_eq!(g.state(), OutputState::Active);
    drain(&mut s);
    s.begin_stop();
    d.run_blocks(4); // 1024 frames > 960-frame fade.
    assert_eq!(s.output_stats().state, OutputState::Silent);
    let cap = channel(&drain(&mut s), 1).1;
    let early = cap[..48].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(early > 0.1, "no hard cut");
    // The envelope is linear: the last 10% of the fade is at most 10% of full level.
    let tail = cap[864..960].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(tail < 0.5 * 0.1 + 1e-6, "{tail}");
    assert!(cap[960..].iter().all(|&v| v == 0.0));
    // Restart is refused after stop.
    g.start();
    d.run_blocks(4);
    assert!(channel(&drain(&mut s), 1).1.iter().all(|&v| v == 0.0));
    assert_eq!(s.stop(Duration::from_millis(10)), StopOutcome::FadedOut);
}

#[test]
fn unsupported_requests_are_typed_errors() {
    let backend = FakeBackend::new(FakeConfig::default()).expect("config");
    let open = |f: &dyn Fn(&mut DuplexRequest)| {
        let mut r = DuplexRequest::new(vec![0], 2, max_level());
        f(&mut r);
        backend.open(r).err()
    };
    assert!(matches!(
        open(&|r| r.sample_rate = Some(44_100)),
        Some(AudioError::Unsupported(Unsupported::SampleRate {
            requested: 44_100,
            ..
        }))
    ));
    assert!(matches!(
        open(&|r| r.buffer_frames = Some(128)),
        Some(AudioError::Unsupported(Unsupported::BufferFrames {
            fixed: Some(256),
            ..
        }))
    ));
    assert!(matches!(
        open(&|r| r.input_map = vec![4]),
        Some(AudioError::Unsupported(Unsupported::InputChannel {
            channel: 4,
            ..
        }))
    ));
    assert!(matches!(
        open(&|r| r.output_channels = 3),
        Some(AudioError::Unsupported(Unsupported::OutputChannels { .. }))
    ));
    assert!(matches!(
        open(&|r| r.input_device = DeviceSelector::Id(DeviceId("nope".into()))),
        Some(AudioError::DeviceNotFound { .. })
    ));
    assert!(matches!(
        open(&|r| r.input_map.clear()),
        Some(AudioError::InvalidRequest(_))
    ));
}

#[test]
fn trait_object_with_parked_manual_driver() {
    let fake = FakeBackend::new(FakeConfig::default()).expect("config");
    let backend: Box<dyn Backend> = Box::new(fake.clone());
    let caps = backend.enumerate().expect("caps");
    assert_eq!(caps.len(), 1);
    assert_eq!(caps[0].input.as_ref().map(|c| c.max_channels), Some(4));
    let mut s = backend
        .open(DuplexRequest::new(vec![0], 2, max_level()))
        .expect("open");
    let mut d = fake.take_driver().expect("parked driver");
    assert!(fake.take_driver().is_none());
    d.run_blocks(3);
    assert_eq!(drain(&mut s).len(), 3);
    assert_eq!(s.stop(Duration::from_millis(10)), StopOutcome::NeverEmitted);
}

#[test]
fn thread_drive_delivers_blocks_and_stops_cleanly() {
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        block_frames: 480,
        stop_after_blocks: Some(20),
        ..FakeConfig::default()
    })
    .expect("config");
    let mut s = backend
        .open(DuplexRequest::new(vec![0, 1], 2, max_level()))
        .expect("open");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut blocks = Vec::new();
    while blocks.len() < 20 && Instant::now() < deadline {
        blocks.extend(drain(&mut s));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(blocks.len(), 20);
    assert!(
        blocks
            .iter()
            .enumerate()
            .all(|(i, (h, _))| h.start_sample == i as u64 * 480)
    );
    assert_eq!(
        s.stop(Duration::from_millis(100)),
        StopOutcome::NeverEmitted
    );
}

/// Blocks delivered by `s` within `wait`.
fn delivered(s: &mut DuplexStream, wait: Duration) -> usize {
    let deadline = Instant::now() + wait;
    let mut n = 0;
    while Instant::now() < deadline {
        n += drain(s).len();
        std::thread::sleep(Duration::from_millis(5));
    }
    n
}

/// Whether `s` delivers a block within `within`: a realtime fake thread can take a while to
/// get its first period on a loaded host, so a live stream is waited for, not sampled.
fn delivers(s: &mut DuplexStream, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if !drain(s).is_empty() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn a_stall_silences_running_streams_without_an_error_and_blocks_opens_until_it_ends() {
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        block_frames: 480,
        ..FakeConfig::default()
    })
    .expect("config");
    let req = || DuplexRequest::new(vec![0], 2, max_level());
    let mut s = backend.open(req()).expect("open");
    assert_eq!(s.negotiated().delivery, ac2_audio::Delivery::Device);
    assert!(delivers(&mut s, Duration::from_secs(2)));
    backend.stall(None);
    std::thread::sleep(Duration::from_millis(20));
    drain(&mut s);
    assert_eq!(delivered(&mut s, Duration::from_millis(150)), 0);
    assert!(!s.events().ended, "a stall reports nothing");
    // An open waits for the device, as behind a hung server.
    let b = backend.clone();
    let opener = std::thread::spawn(move || b.open(DuplexRequest::new(vec![0], 2, max_level())));
    std::thread::sleep(Duration::from_millis(100));
    assert!(!opener.is_finished(), "the open waits out the stall");
    backend.restore();
    let mut fresh = opener.join().expect("join").expect("open after the stall");
    assert!(delivers(&mut fresh, Duration::from_secs(2)));
    assert_eq!(
        delivered(&mut s, Duration::from_millis(50)),
        0,
        "the stream that hung stays silent"
    );
}

#[test]
fn a_vanished_device_ends_its_streams_refuses_opens_and_comes_back_after_its_time() {
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        block_frames: 480,
        ..FakeConfig::default()
    })
    .expect("config");
    let req = || DuplexRequest::new(vec![0], 2, max_level());
    let s = backend.open(req()).expect("open");
    backend.vanish(Some(Duration::from_millis(300)));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !s.events().ended && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(s.events().ended, "the host ends the stream");
    match backend.open(req()) {
        Err(ac2_audio::AudioError::Unavailable { .. }) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(backend.outage(), Some(ac2_audio::OutageKind::Vanish));
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(backend.outage(), None);
    let mut back = backend.open(req()).expect("back after its time");
    assert!(delivers(&mut back, Duration::from_secs(2)));
}

#[test]
fn a_manual_stream_is_stepped() {
    let backend = FakeBackend::new(FakeConfig::default()).expect("config");
    let s = backend
        .open(DuplexRequest::new(vec![0], 2, max_level()))
        .expect("open");
    assert_eq!(s.negotiated().delivery, ac2_audio::Delivery::Stepped);
}

/// Split endpoints list an input-only and an output-only device; a stream opens only with
/// each direction on its own device, and reports no shared clock.
#[test]
fn split_endpoints_open_input_and_output_on_two_devices() {
    use ac2_audio::fake::{FAKE_INPUT_ID, FAKE_OUTPUT_ID};
    use ac2_audio::{ClockRelation, FakeEndpoints};
    let backend = FakeBackend::new(FakeConfig {
        endpoints: FakeEndpoints::Split,
        ..FakeConfig::default()
    })
    .expect("config");
    let listed = backend.enumerate().expect("enumerate");
    let ids: Vec<&str> = listed.iter().map(|d| d.id.0.as_str()).collect();
    assert_eq!(ids, [FAKE_INPUT_ID, FAKE_OUTPUT_ID]);
    assert!(listed[0].input.is_some() && listed[0].output.is_none());
    assert!(listed[1].input.is_none() && listed[1].output.is_some());
    assert!(
        listed
            .iter()
            .all(|d| d.duplex_clock == ClockRelation::Unknown)
    );

    let id = |s: &str| DeviceSelector::Id(DeviceId(s.into()));
    let req = |output: &str| {
        let mut r = DuplexRequest::new(vec![0, 1], 2, max_level());
        r.input_device = id(FAKE_INPUT_ID);
        r.output_device = id(output);
        r
    };
    assert!(matches!(
        backend.open_manual(req(FAKE_INPUT_ID)),
        Err(AudioError::DeviceNotFound { .. })
    ));
    let (stream, _driver) = backend.open_manual(req(FAKE_OUTPUT_ID)).expect("open");
    let n = stream.negotiated();
    assert_eq!(n.input_device.0, FAKE_INPUT_ID);
    assert_eq!(n.output_device.0, FAKE_OUTPUT_ID);
    assert_eq!(n.clock, ClockRelation::Unknown);
}
