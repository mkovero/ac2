//! Loudness bars (`docs/design/programme-meter.md` §6.2) through the daemon: under a loudness
//! ballistic each source's momentary and short-term in the frames read EBU Tech 3341 cases
//! 1, 2, 9, 12 and the live form of 14 played through the fake rig, and every input's own
//! column is its true peak and true-peak hold, a mic's included.
//!
//! The programmes are the generator's 1 kHz sine with its level stepped at block boundaries.
//! Every level change of the generator is a linear 20 ms amplitude ramp, never a step: that
//! is the one difference from the EBU signals. Each case is therefore checked twice: against
//! the mean square of that ramped envelope over the meter's window (the meter's accuracy,
//! ±0.03 LU), and against the published reading within the case's ±0.1 LU (which the ramps
//! leave room for).
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_core::generator::RAMP_SECONDS;
use ac2_proto::frame::{FrameData, LoudnessSource, SessionLevelsFrame, SourceLoudness};
use ac2_proto::model::{
    Ballistic, GeneratorDesired, GeneratorSettings, InputKind, InputSetup, Pair, Signal, State,
};
use ac2_proto::units::{Dbfs, Hz, LeaseToken};
use ac2_proto::{Command, ReplyBody, Subscription};
use ac2d::{Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(10);
/// 10 ms: every segment edge of the cases below is a whole number of blocks, and a block is
/// one of the meter's sub-blocks.
const B: u32 = 480;
const PAIR: LoudnessSource = LoudnessSource::Pair { left: 0, right: 1 };
const MONO: LoudnessSource = LoudnessSource::Input { channel: 2 };
/// The meter's windows in samples at 48 kHz.
const MOMENTARY: usize = 19_200;
const SHORT_TERM: usize = 144_000;
/// Silence for the generator: 80 dB under the quietest tone, below anything a window reads.
const SILENT: f64 = -120.0;
/// A single channel weighs 1 where a pair sums two: 10 log₁₀ 2 under the pair.
const MONO_UNDER_PAIR: f64 = 3.0103;

/// Blocks of `s` seconds.
fn blocks(s: f64) -> u64 {
    (s * f64::from(FS) / f64::from(B)).round() as u64
}

struct Prog {
    _h: Handle,
    c: Client,
    s: Sub,
    d: FakeDriver,
    lease: LeaseToken,
    /// Every level the generator was set to: the block it applies from, dBFS.
    steps: Vec<(u64, f64)>,
    /// Per column, the highest true peak any frame carried so far, dBTP.
    tp_seen: Vec<f32>,
    /// The block a case's programme starts at: the sources start with the frame after
    /// `session.inputs`, so a case begins once that frame is out, with room to spare.
    t0: u64,
}

/// Outputs 0 and 1 looped into the line pair 0+1, output 0 into the unpaired line input 2,
/// output 1 into input 3 (undeclared, so a mic). The generator fires `start` dBFS of 1 kHz on
/// both outputs; the rig's ballistic is `ballistic`.
fn open(name: &str, ballistic: Ballistic, start: f64) -> Prog {
    init_log();
    let backend = FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: B,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 11,
        paths: vec![
            FakePath::loopback(0, 0, 1),
            FakePath::loopback(1, 1, 1),
            FakePath::loopback(0, 2, 1),
            FakePath::loopback(1, 3, 1),
        ],
        ..FakeConfig::default()
    })
    .unwrap();
    let h = Daemon::start(steady_config(backend.clone(), inproc(name))).unwrap();
    let (mut c, s) = connect(&h, &[&Subscription::InputMeters.prefix()]);
    let mut cfg = session(false);
    cfg.input_channels = vec![0, 1, 2, 3];
    cfg.sample_rate_hz = Some(FS);
    cfg.buffer_frames = Some(B);
    c.ok(Command::SessionOpen { config: cfg });
    let line = |channel| InputSetup {
        channel,
        kind: InputKind::Line,
        pair: None,
    };
    c.ok(Command::SessionInputs {
        inputs: vec![line(0), line(1), line(2)],
        pairs: vec![Pair {
            left: 0,
            right: 1,
            name: None,
        }],
    });
    let base = match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => {
            let st: State = s.state;
            st.rig.rev
        }
        other => panic!("{other:?}"),
    };
    c.ok(Command::LevelsSet {
        base,
        ballistic: Some(ballistic),
        correlation_window: None,
        over_threshold: None,
        correlation_gate: None,
        loudness_target: None,
    });
    let ReplyBody::Lease(lease) = c.ok(Command::GenAcquire { force: false }) else {
        panic!("lease");
    };
    let d = driver(&backend);
    let mut p = Prog {
        _h: h,
        c,
        s,
        d,
        lease: lease.lease_token,
        steps: Vec::new(),
        tp_seen: vec![f32::NEG_INFINITY; 4],
        t0: 0,
    };
    p.level(start);
    p.frame(1);
    p.t0 = p.d.blocks() + 10;
    p
}

impl Prog {
    /// The generator at `db` from the next block on.
    fn level(&mut self, db: f64) {
        self.c.ok(Command::GenSet {
            lease_token: self.lease,
            desired: GeneratorDesired {
                settings: GeneratorSettings {
                    signal: Signal::Sine { freq: Hz(1000.0) },
                    level: Dbfs(db),
                    band: None,
                    outputs: vec![0, 1],
                },
                armed: true,
                firing: true,
            },
        });
        self.steps.push((self.d.blocks(), db));
    }

    fn run(&mut self, mut n: u64) {
        while n > 0 {
            let k = n.min(48);
            self.d.run_blocks(k);
            taken(&self.d);
            n -= k;
        }
    }

    /// The meters up to the end of block `upto` (exclusive count): one block run after the
    /// frame interval, so the fan-out has a frame due and publishes what it metered.
    fn frame(&mut self, upto: u64) -> SessionLevelsFrame {
        assert_eq!(self.d.blocks() + 1, upto, "a read is one block ahead");
        std::thread::sleep(Duration::from_millis(40));
        self.run(1);
        let end = upto * u64::from(B);
        let seen = &mut self.tp_seen;
        let f = self
            .s
            .frame(T, |f| {
                let FrameData::SessionLevels(l) = &f.data else {
                    return false;
                };
                for (s, &tp) in seen.iter_mut().zip(&l.true_peak) {
                    *s = s.max(tp);
                }
                f.stamp.audio_sample.0 + 1 >= end
            })
            .unwrap_or_else(|| panic!("no meter frame up to sample {end}"));
        let FrameData::SessionLevels(l) = f.data else {
            unreachable!()
        };
        l
    }

    /// Plays `steps` (block, dBFS: the level from that block on) and reads a frame at the end
    /// of each block of `reads`, in block order.
    fn play(&mut self, steps: &[(u64, f64)], reads: &[u64]) -> Vec<(u64, SessionLevelsFrame)> {
        let (mut si, mut ri) = (0, 0);
        let mut got = Vec::new();
        loop {
            while si < steps.len() && steps[si].0 == self.d.blocks() {
                self.level(steps[si].1);
                si += 1;
            }
            assert!(
                si == steps.len() || steps[si].0 > self.d.blocks(),
                "a step in the past"
            );
            if ri < reads.len() && reads[ri] == self.d.blocks() + 1 {
                got.push((reads[ri], self.frame(reads[ri])));
                ri += 1;
                continue;
            }
            let next_step = steps.get(si).map(|s| s.0);
            let next_read = reads.get(ri).map(|r| r - 1);
            let Some(next) = [next_step, next_read].into_iter().flatten().min() else {
                return got;
            };
            self.run(next - self.d.blocks());
        }
    }

    /// Under a loudness ballistic the frame says so, and every input's own column, a mic's
    /// included, is its true peak and true-peak hold.
    fn columns_are_true_peak(&self, f: &SessionLevelsFrame, ballistic: Ballistic, held: bool) {
        assert_eq!(f.meta.ballistic, ballistic);
        assert_eq!(f.meta.channels, [0, 1, 2, 3]);
        for i in 0..4 {
            assert_eq!(f.ppm[i], f.true_peak[i], "input {i}: {f:?}");
            assert!(f.max[i] >= f.true_peak[i], "input {i}: {f:?}");
            if held {
                assert!(
                    (f.max[i] - self.tp_seen[i]).abs() < 1e-3,
                    "input {i}: max {} is the highest true peak {}",
                    f.max[i],
                    self.tp_seen[i]
                );
            }
        }
        assert!(f.true_peak[3].is_finite(), "the mic hears the tone");
    }

    /// The generator's envelope (linear amplitude, 1 = 0 dBFS) at every session sample up to
    /// `end`, as it ramps: from the block of each step, linearly over the ramp.
    fn envelope(&self, end: usize) -> Vec<f64> {
        let ramp = (RAMP_SECONDS * f64::from(FS)).round() as usize;
        let mut g = vec![0.0; end];
        let (mut gain, mut step, mut left) = (0.0_f64, 0.0, 0_usize);
        let mut steps = self.steps.iter().peekable();
        for (n, v) in g.iter_mut().enumerate() {
            while let Some(&&(b, db)) = steps.peek() {
                if (b * u64::from(B)) as usize != n {
                    break;
                }
                let target = 10f64.powf(db / 20.0);
                if n == 0 {
                    // The first fire fades in on its own; no window read here reaches it.
                    gain = target;
                } else {
                    step = (target - gain) / ramp as f64;
                    left = ramp;
                }
                steps.next();
            }
            if left > 0 {
                left -= 1;
                gain += step;
            }
            *v = gain;
        }
        g
    }
}

/// The loudness (pair, LUFS) of the envelope over the `window` samples ending at `end`.
fn window_lufs(g: &[f64], end: usize, window: usize) -> f64 {
    let ms: f64 = g[end - window..end].iter().map(|a| a * a).sum::<f64>() / window as f64;
    10.0 * ms.log10()
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

/// The meter against the ramped envelope (±0.03 LU) and the published reading (±0.1 LU).
fn reads_as(what: &str, got: f64, model: f64, published: f64) {
    near(&format!("{what} (envelope)"), got, model, 0.03);
    near(&format!("{what} (Tech 3341)"), got, published, 0.1);
}

/// Cases 1 and 2: −23 and then −33 dBFS on a line pair read −23.0 / −33.0 LUFS momentary
/// (after 400 ms) and short-term (after 3 s), and on one input of the signal 3 LU lower.
#[test]
fn cases_1_and_2_on_a_pair_and_one_input() {
    let mut p = open("bars-3341-1-2", Ballistic::ShortTerm, -23.0);
    let t0 = p.t0;
    let change = t0 + blocks(3.6);
    let reads: Vec<u64> = (0..15)
        .map(|k| t0 + blocks(0.5) + 20 * k)
        .chain((0..15).map(|k| change + blocks(0.45) + 20 * k))
        .collect();
    let got = p.play(&[(change, -33.0)], &reads);
    for (at, f) in &got {
        let (level, since) = if *at < change {
            (-23.0, t0)
        } else {
            (-33.0, change + 2)
        };
        let after = (at - since) as f64 * f64::from(B) / f64::from(FS);
        let (pr, mo) = (source(f, PAIR), source(f, MONO));
        near("pair momentary", pr.momentary.0, level, 0.1);
        near(
            "mono momentary",
            mo.momentary.0,
            level - MONO_UNDER_PAIR,
            0.1,
        );
        if after > 3.0 {
            near("pair short-term", pr.short_term.0, level, 0.1);
            near(
                "mono short-term",
                mo.short_term.0,
                level - MONO_UNDER_PAIR,
                0.1,
            );
        }
        p.columns_are_true_peak(f, Ballistic::ShortTerm, true);
    }
    // After the change the bars' columns read −33's true peak while the holds keep −23's.
    let (_, last) = got.last().unwrap();
    assert!(last.max[0] - last.ppm[0] > 9.8, "{last:?}");
}

/// Case 9: 1.34 s at −20 and 1.66 s at −30 dBFS, five times: every short-term after 3 s
/// reads −23.0 LUFS, whatever the window's phase (it spans one period).
#[test]
fn case_9_short_term_over_a_period() {
    let mut p = open("bars-3341-9", Ballistic::ShortTerm, -20.0);
    let (hi, lo) = (blocks(1.34), blocks(1.66));
    let t0 = p.t0;
    let mut steps = Vec::new();
    for k in 0..5 {
        let at = t0 + k * (hi + lo);
        if k > 0 {
            steps.push((at, -20.0));
        }
        steps.push((at + hi, -30.0));
    }
    // Each read 37 blocks on: the windows land on many phases of the period.
    let reads: Vec<u64> = (t0 + blocks(3.0) + 1..t0 + 5 * (hi + lo))
        .step_by(37)
        .collect();
    let got = p.play(&steps, &reads);
    let g = p.envelope(((t0 + 5 * (hi + lo)) * u64::from(B)) as usize);
    for (at, f) in &got {
        let end = (*at * u64::from(B)) as usize;
        let model = window_lufs(&g, end, SHORT_TERM);
        reads_as(
            &format!("short-term at block {at}"),
            source(f, PAIR).short_term.0,
            model,
            -23.0,
        );
        p.columns_are_true_peak(f, Ballistic::ShortTerm, true);
    }
}

/// Case 12: 0.18 s at −20 and 0.22 s at −30 dBFS, 25 times: every momentary after 1 s reads
/// −23.0 LUFS.
#[test]
fn case_12_momentary_over_a_period() {
    let mut p = open("bars-3341-12", Ballistic::Momentary, -20.0);
    let (hi, lo) = (blocks(0.18), blocks(0.22));
    let t0 = p.t0;
    let mut steps = Vec::new();
    for k in 0..25 {
        let at = t0 + k * (hi + lo);
        if k > 0 {
            steps.push((at, -20.0));
        }
        steps.push((at + hi, -30.0));
    }
    let reads: Vec<u64> = (t0 + blocks(1.0) + 1..t0 + 25 * (hi + lo))
        .step_by(23)
        .collect();
    let got = p.play(&steps, &reads);
    let g = p.envelope(((t0 + 25 * (hi + lo)) * u64::from(B)) as usize);
    for (at, f) in &got {
        let end = (*at * u64::from(B)) as usize;
        let model = window_lufs(&g, end, MOMENTARY);
        reads_as(
            &format!("momentary at block {at}"),
            source(f, PAIR).momentary.0,
            model,
            -23.0,
        );
        p.columns_are_true_peak(f, Ballistic::Momentary, true);
    }
}

/// Case 14, live: after a reset, twenty 0.8 s segments, the i-th holding a 0.4 s tone at
/// −38 + i dBFS after 20·i ms of silence. At each segment's end the max momentary since the
/// reset, what a bar's max ticks show, reads −38 + i LUFS.
#[test]
fn case_14_max_momentary_at_each_segment_end() {
    let mut p = open("bars-3341-14", Ballistic::Momentary, SILENT);
    p.run(blocks(0.5) - 1);
    p.c.ok(Command::LevelsReset {
        channels: Vec::new(),
    });
    // The reset reaches the meters with the next frame.
    let upto = p.d.blocks() + 1;
    p.frame(upto);
    let start = p.d.blocks() + blocks(0.2);
    let (seg, tone) = (blocks(0.8), blocks(0.4));
    let mut steps = Vec::new();
    let mut reads = Vec::new();
    for i in 0..20u64 {
        let on = start + seg * i + 2 * i;
        // A ramp is centred on the edge it stands for: it starts half a ramp (a block) early.
        steps.push((on - 1, -38.0 + i as f64));
        steps.push((on + tone - 1, SILENT));
        reads.push(start + seg * (i + 1));
    }
    let got = p.play(&steps, &reads);
    let end = (*reads.last().unwrap() * u64::from(B)) as usize;
    let g = p.envelope(end);
    let sub = B as usize;
    for (i, (at, f)) in got.iter().enumerate() {
        let to = (*at * u64::from(B)) as usize;
        let from = (start * u64::from(B)) as usize;
        let model = (from / sub..=to / sub)
            .map(|k| k * sub)
            .filter(|&e| e >= from + MOMENTARY)
            .map(|e| window_lufs(&g, e, MOMENTARY))
            .fold(f64::NEG_INFINITY, f64::max);
        reads_as(
            &format!("max momentary after segment {i}"),
            source(f, PAIR).max_momentary.0,
            model,
            -38.0 + i as f64,
        );
        p.columns_are_true_peak(f, Ballistic::Momentary, false);
    }
}
