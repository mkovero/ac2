//! Programme loudness (`docs/design/programme-meter.md` §6.1) through the daemon: which
//! inputs carry a source, the reading of a calibration tone, `levels.reset`, a capture gap
//! and a rate BS.1770 is not defined for. The fake rig is driven by hand, so seconds of
//! programme take milliseconds and every frame is placed on a known sample.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_audio::fake::{FakeDrive, FakeFault, FakePath};
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::frame::{FrameData, LoudnessSource, SessionLevelsFrame, SourceLoudness};
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, InputKind, InputSetup, Pair, Signal};
use ac2_proto::units::{Dbfs, Hz};
use ac2_proto::{Command, ReplyBody, Subscription};
use ac2d::{Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(10);
const PAIR: LoudnessSource = LoudnessSource::Pair { left: 0, right: 1 };
const MONO: LoudnessSource = LoudnessSource::Input { channel: 2 };

/// Outputs 0 and 1 looped into inputs 0 and 1, output 0 also into input 2; input 3 hears
/// nothing.
fn tone_rig(sample_rate: u32, faults: Vec<FakeFault>) -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 7,
        paths: vec![
            FakePath::loopback(0, 0, 1),
            FakePath::loopback(1, 1, 1),
            FakePath::loopback(0, 2, 1),
        ],
        faults,
        ..FakeConfig::default()
    })
    .unwrap()
}

fn line(channel: u16) -> InputSetup {
    InputSetup {
        channel,
        kind: InputKind::Line,
        pair: None,
    }
}

struct Rig {
    _h: Handle,
    c: Client,
    s: Sub,
    d: FakeDriver,
    sample_rate: u32,
    /// Frames the device lost in a scripted xrun, once the driver has passed it.
    gap: (u64, u64),
}

/// A session on every input of `backend`: 0+1 a line pair, 2 an unpaired line input, 3
/// undeclared (a mic). With `tone`, a −18 dBFS 1 kHz sine on both outputs.
fn open(name: &str, backend: FakeBackend, sample_rate: u32, tone: bool) -> Rig {
    init_log();
    let h = Daemon::start(steady_config(backend.clone(), inproc(name))).unwrap();
    let (mut c, s) = connect(&h, &[&Subscription::InputMeters.prefix()]);
    let mut cfg = session(false);
    cfg.input_channels = vec![0, 1, 2, 3];
    cfg.sample_rate_hz = Some(sample_rate);
    c.ok(Command::SessionOpen { config: cfg });
    c.ok(Command::SessionInputs {
        inputs: vec![line(0), line(1), line(2)],
        pairs: vec![Pair {
            left: 0,
            right: 1,
            name: None,
        }],
    });
    if tone {
        let ReplyBody::Lease(lease) = c.ok(Command::GenAcquire { force: false }) else {
            panic!("lease");
        };
        c.ok(Command::GenSet {
            lease_token: lease.lease_token,
            desired: GeneratorDesired {
                settings: GeneratorSettings {
                    signal: Signal::Sine { freq: Hz(1000.0) },
                    level: Dbfs(-18.0),
                    band: None,
                    outputs: vec![0, 1],
                },
                armed: true,
                firing: true,
            },
        });
    }
    let d = driver(&backend);
    Rig {
        _h: h,
        c,
        s,
        d,
        sample_rate,
        gap: (u64::MAX, 0),
    }
}

impl Rig {
    fn seconds(&mut self, s: f64) {
        self.blocks((s * f64::from(self.sample_rate) / f64::from(BLOCK)).ceil() as u64);
    }

    fn blocks(&mut self, mut n: u64) {
        while n > 0 {
            let k = n.min(16);
            self.d.run_blocks(k);
            taken(&self.d);
            n -= k;
        }
    }

    /// One past the newest session sample: the device's blocks plus a gap it skipped.
    fn end(&self) -> u64 {
        let (at, lost) = self.gap;
        end_sample(&self.d) + if self.d.blocks() > at { lost } else { 0 }
    }

    /// The meters as of now: one more block after the frame interval, so the fan-out has
    /// a frame due and publishes what it has metered up to that block.
    fn now(&mut self) -> SessionLevelsFrame {
        std::thread::sleep(Duration::from_millis(40));
        self.blocks(1);
        let end = self.end();
        let f = self
            .s
            .frame(T, |f| {
                matches!(f.data, FrameData::SessionLevels(_)) && f.stamp.audio_sample.0 + 1 >= end
            })
            .unwrap_or_else(|| panic!("no meter frame up to sample {end}"));
        let FrameData::SessionLevels(l) = f.data else {
            unreachable!()
        };
        l
    }
}

fn source(f: &SessionLevelsFrame, s: LoudnessSource) -> SourceLoudness {
    *f.meta
        .loudness
        .iter()
        .find(|l| l.source == s)
        .unwrap_or_else(|| panic!("no source {s:?} in {:?}", f.meta.loudness))
}

fn near(what: &str, got: f64, want: f64, tol: f64) {
    assert!(
        (got - want).abs() <= tol,
        "{what}: {got}, want {want} ± {tol}"
    );
}

/// A −18 dBFS stereo 1 kHz tone on a line pair reads −18 LUFS, the same tone on one
/// unpaired input reads 3 LU lower (BS.1770 weighs a single channel 1), and a mic input
/// has no source.
#[test]
fn a_calibration_tone_reads_its_level_on_a_pair_and_3_lu_lower_mono() {
    let mut r = open("loudness-tone", tone_rig(FS, Vec::new()), FS, true);
    // The generator's fade-in is over before the reset, so integrated sees the steady tone.
    r.seconds(0.5);
    r.c.ok(Command::LevelsReset {
        channels: Vec::new(),
    });
    let before = wall_now_ns();
    // The reset reaches the meters with the next frame.
    r.now();
    let reset_at = r.d.blocks();
    r.seconds(4.0);
    let f = r.now();
    let metered = (r.d.blocks() - reset_at) as f64 * f64::from(BLOCK) / f64::from(FS);
    let sources: Vec<_> = f.meta.loudness.iter().map(|l| l.source).collect();
    assert_eq!(sources, [PAIR, MONO], "pairs first, then unpaired; no mic");
    let p = source(&f, PAIR);
    near("pair integrated", p.integrated.unwrap().0, -18.0, 0.1);
    near("pair momentary", p.momentary.0, -18.0, 0.1);
    near("pair short-term", p.short_term.0, -18.0, 0.1);
    near("pair max momentary", p.max_momentary.0, -18.0, 0.1);
    near("pair max short-term", p.max_short_term.0, -18.0, 0.1);
    near("pair max true peak", p.max_true_peak.0, -18.0, 0.2);
    near("pair range", p.range.unwrap().0, 0.0, 0.1);
    near("pair duration", p.duration.0, metered, 0.011);
    assert!(
        p.since.0 + 1_000_000_000 >= before,
        "since is the reset's time"
    );
    let m = source(&f, MONO);
    near("mono integrated", m.integrated.unwrap().0, -21.01, 0.1);
    near("mono momentary", m.momentary.0, -21.01, 0.1);
}

/// A reset of one input of a pair restarts the pair (integrated, maxima, duration, since)
/// and leaves the other source alone; momentary is signal state and keeps reading.
#[test]
fn a_reset_restarts_the_sources_of_its_inputs_and_momentary_keeps_reading() {
    let mut r = open("loudness-reset", tone_rig(FS, Vec::new()), FS, true);
    r.seconds(2.0);
    let f = r.now();
    let (p0, m0) = (source(&f, PAIR), source(&f, MONO));
    assert!(p0.integrated.is_some() && p0.duration.0 > 1.5, "{p0:?}");
    std::thread::sleep(Duration::from_millis(5));
    r.c.ok(Command::LevelsReset { channels: vec![1] });
    // The reset reaches the meters with the next frame and the next sub-block boundary.
    r.now();
    r.seconds(0.2);
    let f = r.now();
    let (p, m) = (source(&f, PAIR), source(&f, MONO));
    assert!(p.since.0 > p0.since.0, "the pair restarted: {p:?}");
    assert!(p.duration.0 < 0.35, "{p:?}");
    assert_eq!(p.integrated, None, "no gating block yet after the reset");
    assert_eq!(
        p.max_momentary.0,
        f64::NEG_INFINITY,
        "no window wholly after the reset"
    );
    near("pair momentary across the reset", p.momentary.0, -18.0, 0.1);
    assert_eq!(m.since, m0.since, "input 2 was not reset");
    assert!(m.duration.0 > p0.duration.0, "{m:?}");
    assert!(m.integrated.is_some());
    r.seconds(1.0);
    let p = source(&r.now(), PAIR);
    near(
        "pair integrated after the reset",
        p.integrated.unwrap().0,
        -18.0,
        0.1,
    );
}

/// An xrun that loses a second of capture: the windows restart after the gap (momentary
/// reads −∞ for 400 ms, short-term for 3 s), integrated carries on and the duration counts
/// metered samples only.
#[test]
fn a_capture_gap_restarts_the_windows_and_keeps_integrated() {
    const XRUN_BLOCK: u64 = 400;
    const LOST: u32 = 48_000;
    let faults = vec![FakeFault::Xrun {
        at_block: XRUN_BLOCK,
        lost_frames: LOST,
    }];
    let mut r = open("loudness-gap", tone_rig(FS, faults), FS, true);
    r.gap = (XRUN_BLOCK, u64::from(LOST));
    r.seconds(0.5);
    r.c.ok(Command::LevelsReset {
        channels: Vec::new(),
    });
    r.now();
    let reset_at = r.d.blocks();
    // The last block before the gap is the one `now` runs.
    let to_gap = XRUN_BLOCK - 1 - r.d.blocks();
    r.blocks(to_gap);
    let f = r.now();
    let p0 = source(&f, PAIR);
    near("momentary before the gap", p0.momentary.0, -18.0, 0.1);
    // 0.2 s after the resume.
    r.seconds(0.2);
    let f = r.now();
    let p = source(&f, PAIR);
    assert_eq!(p.momentary.0, f64::NEG_INFINITY, "{p:?}");
    assert_eq!(p.short_term.0, f64::NEG_INFINITY, "{p:?}");
    near(
        "integrated across the gap",
        p.integrated.unwrap().0,
        -18.0,
        0.1,
    );
    let metered = (r.d.blocks() - reset_at) as f64 * f64::from(BLOCK) / f64::from(FS);
    near("duration without the gap", p.duration.0, metered, 0.011);
    r.seconds(0.3);
    let p = source(&r.now(), PAIR);
    near("momentary after 400 ms", p.momentary.0, -18.0, 0.1);
    assert_eq!(p.short_term.0, f64::NEG_INFINITY, "{p:?}");
}

/// At 32 kHz (below BS.1770's range here) the meters run and there is no loudness source.
#[test]
fn no_loudness_sources_at_an_unsupported_rate() {
    let fs = 32_000;
    let mut r = open("loudness-32k", tone_rig(fs, Vec::new()), fs, false);
    r.seconds(0.5);
    let f = r.now();
    assert_eq!(f.meta.channels, [0, 1, 2, 3]);
    assert_eq!(f.meta.pair_levels.len(), 1);
    assert!(f.meta.loudness.is_empty(), "{:?}", f.meta.loudness);
}
