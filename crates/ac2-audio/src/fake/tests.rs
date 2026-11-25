use crate::backend::{Backend, DeviceId, DeviceSelector, Direction, Presence};

use super::analog::{SINC_HALF, SINC_PHASES, sinc_table};
use super::*;

/// The probe sees what a server socket would show: gone while vanished, there while
/// stalled, a new generation once back; another device is never there.
#[test]
fn probe_follows_the_outages() {
    let b = FakeBackend::new(FakeConfig::default()).expect("fake");
    let any = DeviceSelector::Default;
    let p0 = b.probe(&any);
    assert!(matches!(p0, Presence::Present { .. }));
    b.vanish(None);
    assert_eq!(b.probe(&any), Presence::Absent);
    b.restore();
    let p1 = b.probe(&any);
    assert!(matches!(p1, Presence::Present { .. }) && p1 != p0);
    b.stall(None);
    let p2 = b.probe(&any);
    assert!(matches!(p2, Presence::Present { .. }) && p2 != p1);
    b.restore();
    assert!(b.probe(&any) != p2);
    let other = DeviceSelector::Id(DeviceId("elsewhere".into()));
    assert_eq!(b.probe(&other), Presence::Absent);
}

#[test]
fn config_validation() {
    let bad = |c: FakeConfig| FakeBackend::new(c).err();
    assert_eq!(
        bad(FakeConfig {
            block_frames: 0,
            ..FakeConfig::default()
        }),
        Some(FakeConfigError::Zero("block_frames"))
    );
    assert!(matches!(
        bad(FakeConfig {
            paths: vec![FakePath::loopback(2, 0, 0)],
            ..FakeConfig::default()
        }),
        Some(FakeConfigError::PathChannel {
            direction: Direction::Output,
            ..
        })
    ));
    assert_eq!(
        bad(FakeConfig {
            faults: vec![
                FakeFault::DropOutputFrames {
                    at_output_sample: 100,
                    frames: 10
                },
                FakeFault::RepeatOutputFrames {
                    at_output_sample: 105,
                    frames: 10
                },
            ],
            ..FakeConfig::default()
        }),
        Some(FakeConfigError::OutputFaults)
    );
    assert!(
        bad(FakeConfig {
            drift_ppm: f64::NAN,
            ..FakeConfig::default()
        })
        .is_some()
    );
}

#[test]
fn sinc_rows_have_unit_dc_gain_and_integer_phase_is_a_unit_impulse() {
    let t = sinc_table();
    let taps = 2 * SINC_HALF;
    for p in [0, 1, SINC_PHASES / 2, SINC_PHASES] {
        let s: f32 = t[p * taps..(p + 1) * taps].iter().sum();
        assert!((s - 1.0).abs() < 1e-5);
    }
    let row0 = &t[..taps];
    assert!((row0[SINC_HALF - 1] - 1.0).abs() < 1e-6);
    assert!(
        row0.iter()
            .enumerate()
            .all(|(k, v)| k == SINC_HALF - 1 || v.abs() < 1e-6)
    );
}

/// The start-up stamp error is the stated offset at the first frame and gone a few time
/// constants on; without it every stamp is host time.
#[test]
fn a_start_up_stamp_error_decays_away() {
    let rate = 48_000_u64;
    let cfg = FakeConfig {
        start_stamp_error: Some(StartStampError {
            offset_ns: 2e6,
            decay_s: 0.5,
        }),
        ..FakeConfig::default()
    };
    let err = |f: u64| cfg.capture_ns(f) as i64 - cfg.host_ns(f) as i64;
    assert_eq!(err(0), 2_000_000);
    assert!((err(rate / 2) - 735_759).abs() <= 1, "{}", err(rate / 2));
    assert_eq!(err(10 * rate), 0);
    let exact = FakeConfig::default();
    assert_eq!(exact.capture_ns(rate), exact.host_ns(rate));
    assert_eq!(
        FakeBackend::new(FakeConfig {
            start_stamp_error: Some(StartStampError {
                offset_ns: 1e6,
                decay_s: 0.0,
            }),
            ..FakeConfig::default()
        })
        .err(),
        Some(FakeConfigError::BadNumber("start_stamp_error"))
    );
}
