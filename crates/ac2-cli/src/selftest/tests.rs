use ac2_audio::fake::{FakeDrive, FakeFault, FakePath};
use ac2_audio::{BlockFlags, BlockHeader, DeviceSelector, FakeBackend, FakeConfig, FakeDriver};

use super::stats::StreamStats;
use super::*;

const RATE: u32 = 48_000;
const BLOCK: u32 = 256;

fn backend(paths: Vec<FakePath>, drift_ppm: f64, faults: Vec<FakeFault>) -> FakeBackend {
    FakeBackend::new(FakeConfig {
        drive: FakeDrive::Manual,
        sample_rate: RATE,
        block_frames: BLOCK,
        inputs: 2,
        outputs: 2,
        paths,
        drift_ppm,
        faults,
        ..FakeConfig::default()
    })
    .expect("fake")
}

fn spec() -> DuplexSpec {
    DuplexSpec {
        input_device: DeviceSelector::Default,
        output_device: DeviceSelector::Default,
        input_map: vec![0, 1],
        output_channels: 2,
        sample_rate: None,
        buffer_frames: None,
    }
}

fn emit(level_dbfs: f64) -> Emit {
    Emit {
        level_dbfs,
        loopback_out: 0,
        loopback_in: 0,
    }
}

fn open(b: &FakeBackend, emit: Option<Emit>) -> (DuplexSession, FakeDriver) {
    let session = DuplexSession::open(b, &spec(), emit)
        .expect("setup")
        .expect("opened");
    (session, b.take_driver().expect("manual driver"))
}

/// Steps the device `seconds` of device time, draining as a real-time reader would, then
/// fades out and judges.
fn run_for(mut s: DuplexSession, d: &mut FakeDriver, seconds: f64) -> DuplexReport {
    let blocks = (seconds * f64::from(RATE) / f64::from(BLOCK)).ceil() as u64;
    let mut done = 0;
    while done < blocks {
        let n = (blocks - done).min(64);
        d.run_blocks(n);
        s.poll();
        done += n;
    }
    s.begin_stop();
    // The fade-out (20 ms) completes within a few blocks.
    d.run_blocks(16);
    s.finish(false)
}

#[test]
fn a_clean_silent_run_passes_and_reports_the_stream() {
    let b = backend(vec![], 0.0, vec![]);
    let (s, mut d) = open(&b, None);
    let r = run_for(s, &mut d, 3.0);
    assert!(r.pass, "{:?}", r.failures);
    let o = r.opened.as_ref().expect("opened");
    assert_eq!((o.sample_rate, o.device_inputs, o.outputs), (RATE, 2, 2));
    let c = r.capture.as_ref().expect("capture");
    assert!(c.blocks >= 563);
    assert_eq!(c.block_sizes.len(), 1);
    assert_eq!((c.index_gaps, c.xrun_blocks), (0, 0));
    // The fake's timestamps are its own device time: exactly the nominal rate.
    let rate = c.rate.expect("rate judged");
    assert!(rate.ppm.abs() < 1e-3 && rate.span_s > 2.9, "{rate:?}");
    assert_eq!(c.peak_dbfs, [None, None], "silent inputs");
    let out = r.output.as_ref().expect("output");
    assert!(out.callbacks >= 563);
    assert!(out.rate.is_some_and(|r| r.ppm.abs() < 1e-3));
    assert_eq!(r.stop, Some(StopOutcome::NeverEmitted));
    assert!(r.loopback.is_none(), "no timing without a stimulus");
}

#[test]
fn an_injected_xrun_fails_with_named_reasons() {
    let b = backend(
        vec![],
        0.0,
        vec![FakeFault::Xrun {
            at_block: 100,
            lost_frames: 512,
        }],
    );
    let (s, mut d) = open(&b, None);
    let r = run_for(s, &mut d, 1.0);
    assert!(!r.pass);
    assert!(
        r.failures
            .iter()
            .any(|f| matches!(f, Failure::Xruns { count } if *count >= 1)),
        "{:?}",
        r.failures
    );
    assert!(
        r.failures.contains(&Failure::IndexGaps {
            gaps: 1,
            frames: 512
        }),
        "{:?}",
        r.failures
    );
}

#[test]
fn a_config_change_and_an_interruption_fail() {
    let b = backend(vec![], 0.0, vec![FakeFault::ConfigChange { at_block: 10 }]);
    let (mut s, mut d) = open(&b, None);
    d.run_blocks(40);
    s.poll();
    let r = s.finish(true);
    assert!(
        r.failures.contains(&Failure::Interrupted),
        "{:?}",
        r.failures
    );
    assert!(
        r.failures
            .iter()
            .any(|f| matches!(f, Failure::ConfigChanged { .. })),
        "{:?}",
        r.failures
    );
}

#[test]
fn a_device_that_never_delivers_fails() {
    let b = backend(vec![], 0.0, vec![]);
    let (s, _d) = open(&b, None);
    let r = s.finish(false);
    assert!(r.failures.contains(&Failure::NoAudio), "{:?}", r.failures);
    assert!(r.failures.contains(&Failure::NoOutputCallbacks));
}

#[test]
fn emission_above_the_ceiling_or_off_the_stream_is_refused_before_opening() {
    let b = backend(vec![], 0.0, vec![]);
    let refused = |e: Emit| DuplexSession::open(&b, &spec(), Some(e)).err();
    assert!(matches!(refused(emit(-10.0)), Some(SetupError::Level(_))));
    assert_eq!(
        refused(Emit {
            loopback_in: 3,
            ..emit(-40.0)
        }),
        Some(SetupError::LoopbackInNotCaptured(3))
    );
    assert_eq!(
        refused(Emit {
            loopback_out: 2,
            ..emit(-40.0)
        }),
        Some(SetupError::LoopbackOutNotOpened(2))
    );
    assert!(b.take_driver().is_none(), "nothing was opened");
}

#[test]
fn a_loopback_cable_locks_at_its_delay_without_drift() {
    let b = backend(vec![FakePath::loopback(0, 0, 32)], 0.0, vec![]);
    let (s, mut d) = open(&b, Some(emit(-40.0)));
    let r = run_for(s, &mut d, 16.0);
    assert!(r.pass, "{:?}", r.failures);
    let l = r.loopback.as_ref().expect("loopback");
    assert_eq!(l.offset_samples, Some(32));
    assert_eq!((l.locks, l.jumps.len(), l.lost), (1, 0, 0));
    assert!(l.drift_ppm.is_some_and(|p| p.abs() < 0.5), "{l:?}");
    assert_eq!(r.limited_samples, 0);
    assert_eq!(r.stop, Some(StopOutcome::FadedOut));
    let c = r.capture.as_ref().expect("capture");
    assert!(c.peak_dbfs[0].is_some() && c.peak_dbfs[1].is_none());
}

#[test]
fn output_and_input_on_different_clocks_fail_as_drift() {
    // The DAC runs 20 ppm slow, so the loopback reads ever older output: well inside the
    // simulated DAC history.
    let b = backend(vec![FakePath::loopback(0, 0, 64)], -20.0, vec![]);
    let (s, mut d) = open(&b, Some(emit(-40.0)));
    let r = run_for(s, &mut d, 16.0);
    let drift = r.failures.iter().find_map(|f| match f {
        Failure::ClockDrift { ppm } => Some(*ppm),
        _ => None,
    });
    assert!(
        drift.is_some_and(|p| (p - 20.0).abs() < 2.0),
        "{:?}",
        r.failures
    );
}

fn header(start: u64, ns: u64, flags: BlockFlags) -> BlockHeader {
    BlockHeader {
        start_sample: start,
        frames: BLOCK,
        channels: 1,
        flags,
        callback_ns: ns,
        capture_ns: Some(ns),
    }
}

#[test]
fn a_device_clock_off_its_rate_is_measured_across_index_gaps() {
    // The device delivers 48 000 samples per 1.01 host seconds: −9901 ppm. A gap of unknown
    // length in the middle must not bend the line.
    let mut s = StreamStats::new(RATE, 1);
    let ns_per_sample = 1e9 / f64::from(RATE) * 1.01;
    let samples = vec![0.0; BLOCK as usize];
    for i in 0..400u64 {
        let start = i * u64::from(BLOCK) + if i >= 200 { 10_000 } else { 0 };
        let host = (start as f64 * ns_per_sample) as u64 + if i >= 200 { 7_000_000 } else { 0 };
        let flags = if i == 200 {
            BlockFlags::DISCONTINUITY | BlockFlags::GAP_ESTIMATED
        } else {
            BlockFlags::NONE
        };
        s.add_block(&header(start, host, flags), &samples);
    }
    let (c, _) = s.finish();
    let r = c.rate.expect("rate");
    assert!((r.ppm + 9901.0).abs() < 1.0, "{r:?}");
    assert!(r.ppm.abs() > RATE_TOLERANCE_PPM);
    assert_eq!((c.index_gaps, c.gap_frames), (1, 10_000));
    assert_eq!(c.estimated_gap_blocks, 1);
}
