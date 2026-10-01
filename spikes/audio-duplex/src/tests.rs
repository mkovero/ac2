//! End-to-end tests through the trait. Fake-backed tests need no hardware; device tests
//! skip (with a message) when the device or server is absent, as on CI runners.

use std::time::{Duration, Instant};

use crate::backend::{AudioBackend, DuplexRequest, DuplexStream};
use crate::block::{BlockFlags, BlockHeader};
use crate::fake::{FakeBackend, FakeConfig, fake_channel_value};
use crate::output::{EmitLevel, OutputMode};
use crate::stats::RunStats;

fn collect(
    stream: &mut DuplexStream,
    blocks: usize,
    timeout: Duration,
) -> Vec<(BlockHeader, Vec<f32>)> {
    let mut out = Vec::new();
    let mut buf = Vec::new();
    let deadline = Instant::now() + timeout;
    while out.len() < blocks && Instant::now() < deadline {
        match stream.capture.pop_into(&mut buf) {
            Some(h) => out.push((h, buf.clone())),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    out
}

fn fast_fake(cfg: FakeConfig) -> FakeBackend {
    FakeBackend {
        config: FakeConfig {
            realtime: false,
            ..cfg
        },
    }
}

#[test]
fn fake_routes_selected_channels_with_contiguous_indices() {
    let be = fast_fake(FakeConfig {
        max_callbacks: Some(200),
        ..FakeConfig::default()
    });
    let req = DuplexRequest {
        input_map: vec![3, 1],
        ..DuplexRequest::default()
    };
    let mut s = be.open_duplex(&req).expect("open");
    let blocks = collect(&mut s, 200, Duration::from_secs(10));
    assert_eq!(blocks.len(), 200);
    let mut stats = RunStats::new(48_000);
    for (h, data) in &blocks {
        stats.add(h);
        assert_eq!(h.channels, 2);
        for f in 0..h.frames as usize {
            let idx = h.start_sample + f as u64;
            assert_eq!(data[f * 2], fake_channel_value(3, idx));
            assert_eq!(data[f * 2 + 1], fake_channel_value(1, idx));
        }
    }
    let r = stats.report();
    assert_eq!(r.flagged_first, 1);
    assert_eq!(r.index_gaps, 0);
    assert_eq!(r.index_regressions, 0);
    assert_eq!(r.frames, 200 * 256);
    s.stop();
}

#[test]
fn fake_xrun_marks_block_and_advances_index_by_lost_frames() {
    let be = fast_fake(FakeConfig {
        max_callbacks: Some(100),
        xrun_at: Some((50, 512)),
        ..FakeConfig::default()
    });
    let mut s = be.open_duplex(&DuplexRequest::default()).expect("open");
    let blocks = collect(&mut s, 100, Duration::from_secs(10));
    assert_eq!(blocks.len(), 100);
    let (h, _) = &blocks[50];
    assert!(
        h.flags
            .contains(BlockFlags::XRUN | BlockFlags::DISCONTINUITY)
    );
    assert!(!h.flags.contains(BlockFlags::GAP_ESTIMATED));
    assert_eq!(h.start_sample, 50 * 256 + 512);
    let flagged = blocks
        .iter()
        .filter(|(h, _)| h.flags.intersects(BlockFlags::BREAKS_CONTINUITY))
        .count();
    assert_eq!(flagged, 1);
}

#[test]
fn fake_loopback_onset_lands_at_the_loopback_delay_index() {
    // The fake device is not hardware, so emitting into it is allowed.
    let level = EmitLevel::new(-20.0).expect("level");
    let be = fast_fake(FakeConfig {
        max_callbacks: Some(40),
        loopback_delay: 1000,
        ..FakeConfig::default()
    });
    let req = DuplexRequest {
        input_map: vec![0],
        output: OutputMode::Tone {
            level,
            freq_hz: 1000.0,
        },
        ..DuplexRequest::default()
    };
    let mut s = be.open_duplex(&req).expect("open");
    let blocks = collect(&mut s, 40, Duration::from_secs(10));
    let first_nonzero = blocks
        .iter()
        .flat_map(|(h, d)| {
            d.iter()
                .enumerate()
                .map(move |(i, v)| (h.start_sample + i as u64, *v))
        })
        .find(|(_, v)| *v != 0.0)
        .map(|(i, _)| i);
    // Generator sample 0 is sin(0) = 0; sample 1 is the first non-zero one.
    assert_eq!(first_nonzero, Some(1000 + 1));
    let ticks: Vec<_> = std::iter::from_fn(|| s.output_ticks.pop().ok()).collect();
    assert!(
        ticks
            .windows(2)
            .all(|w| w[1].start_sample == w[0].start_sample + 256)
    );
    s.stop();
}

#[test]
fn fake_slow_consumer_gets_overflow_flag_not_partial_blocks() {
    // Real-time pacing so blocks keep arriving after the consumer wakes up again.
    let be = FakeBackend {
        config: FakeConfig {
            max_callbacks: Some(100),
            ..FakeConfig::default()
        },
    };
    let req = DuplexRequest {
        ring_seconds: 0.05, // 2400 frames ≈ 9 blocks
        ..DuplexRequest::default()
    };
    let mut s = be.open_duplex(&req).expect("open");
    std::thread::sleep(Duration::from_millis(200));
    let blocks = collect(&mut s, 100, Duration::from_secs(2));
    assert!(!blocks.is_empty());
    let mut stats = RunStats::new(48_000);
    for (h, d) in &blocks {
        stats.add(h);
        assert_eq!(d.len(), h.samples());
    }
    let r = stats.report();
    assert!(r.flagged_overflow >= 1, "{r:?}");
    assert_eq!(r.index_regressions, 0);
    // Every index jump is flagged.
    assert!(r.flagged_discontinuity >= r.index_gaps);
}

#[test]
fn cpal_enumeration_never_fails_hard() {
    let be = crate::cpal_backend::CpalBackend::default();
    match be.enumerate() {
        Ok(devs) if devs.is_empty() => eprintln!("skip: cpal default host lists no devices"),
        Ok(devs) => {
            for d in devs {
                serde_json::to_string(&d).expect("caps serialise");
            }
        }
        Err(e) => eprintln!("skip: cpal enumeration unavailable: {e}"),
    }
}

/// ALSA's `null` PCM discards output and returns silence: exercises the cpal ALSA path end
/// to end without touching hardware.
#[cfg(target_os = "linux")]
#[test]
fn cpal_alsa_null_device_duplex_runs_silently() {
    let be = crate::cpal_backend::CpalBackend {
        host: Some("alsa".into()),
    };
    let has_null = be
        .enumerate()
        .map(|d| d.iter().any(|d| d.id == "null"))
        .unwrap_or(false);
    if !has_null {
        eprintln!("skip: ALSA null PCM not available");
        return;
    }
    let req = DuplexRequest {
        input_device: Some("null".into()),
        output_device: Some("null".into()),
        input_map: vec![0, 1],
        output_channels: 2,
        sample_rate: Some(48_000),
        ..DuplexRequest::default()
    };
    let mut s = match be.open_duplex(&req) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skip: cannot open ALSA null: {e}");
            return;
        }
    };
    let blocks = collect(&mut s, 20, Duration::from_secs(5));
    assert!(!blocks.is_empty(), "no blocks from ALSA null");
    let mut stats = RunStats::new(48_000);
    for (h, _) in &blocks {
        stats.add(h);
    }
    assert_eq!(stats.report().index_regressions, 0);
    s.stop();
}

#[cfg(all(feature = "jack", target_os = "linux"))]
#[test]
fn jack_duplex_is_exactly_contiguous_when_a_server_runs() {
    let be = crate::jack_backend::JackBackend {
        client_name: "ac2-spike-test".into(),
    };
    let Ok(devs) = be.enumerate() else {
        eprintln!("skip: no JACK server");
        return;
    };
    let inputs = devs[0].input.as_ref().map_or(0, |i| i.max_channels);
    let req = DuplexRequest {
        input_map: (0..inputs.clamp(1, 2)).collect(),
        output_channels: 2,
        connect: crate::backend::ConnectPolicy::InputsOnly,
        ..DuplexRequest::default()
    };
    let mut s = be.open_duplex(&req).expect("open jack");
    let blocks = collect(&mut s, 50, Duration::from_secs(5));
    assert_eq!(blocks.len(), 50);
    let mut stats = RunStats::new(s.negotiated.sample_rate);
    for (h, _) in &blocks {
        stats.add(h);
    }
    let r = stats.report();
    assert_eq!(r.index_regressions, 0);
    assert_eq!(r.flagged_gap_estimated, 0);
    s.stop();
}
