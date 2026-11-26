//! The fake device's own clock: the shared signal of true time
//! ([`InputContent::TrueTimeNoise`]), clock steps, stamps that stop, and pacing by the
//! device's clock.

use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakeFault, InputContent, Pace, TrueTimeNoise};
use ac2_audio::{
    Backend, BlockHeader, DuplexRequest, DuplexStream, FakeBackend, FakeConfig, FakeDriver,
    MaxLevel,
};

const RATE: u32 = 48_000;
const BLOCK: u32 = 1024;

fn noise_cfg(ppm: f64, delay_ns: i64) -> FakeConfig {
    FakeConfig {
        sample_rate: RATE,
        block_frames: BLOCK,
        inputs: 2,
        input_content: InputContent::TrueTimeNoise { delay_ns },
        audio_clock_ppm: ppm,
        ..FakeConfig::default()
    }
}

/// A manually driven stream of inputs 0 and 1, and its simulated system clock's origin
/// (true time at the stream's first frame, ns since the Unix epoch).
fn open(cfg: FakeConfig) -> (DuplexStream, FakeDriver, u64) {
    let backend = FakeBackend::new(cfg).expect("config");
    let level = MaxLevel::from_peak_db(-6.0).expect("level");
    let (stream, driver) = backend
        .open_manual(DuplexRequest::new(vec![0, 1], 2, level))
        .expect("open");
    let origin = stream
        .stamp_clock()
        .and_then(|c| c.system_pair())
        .expect("system pair")
        .system_ns;
    (stream, driver, origin)
}

/// Steps `blocks` blocks, draining as it goes; the headers and input 0's samples.
fn run(s: &mut DuplexStream, d: &mut FakeDriver, blocks: u64) -> (Vec<BlockHeader>, Vec<f32>) {
    let (mut heads, mut x, mut buf) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..blocks {
        d.step();
        while let Some(h) = s.capture().pop_into(&mut buf) {
            let n = usize::from(h.channels);
            x.extend((0..h.frames as usize).map(|f| buf[f * n]));
            assert!(
                (0..h.frames as usize).all(|f| buf[f * n] == buf[f * n + 1]),
                "one signal on every input"
            );
            heads.push(h);
        }
    }
    (heads, x)
}

/// The signal at true time `whole + frac` ns since the Unix epoch (`frac` of any sign).
fn reference(g: &mut TrueTimeNoise, whole: u64, frac: f64) -> f32 {
    let w = frac.floor();
    g.at((i128::from(whole) + w as i128) as u64, frac - w)
}

/// Lag `l` (samples, fractional) at which `b[n]` best matches `a[n + l]` over `b[at..at +
/// len]`, searched within ±4 of `near`. The correlation of band-limited signals is itself
/// band-limited, so it is interpolated by a windowed sinc over ±16 lags and its peak found
/// on a 0.001-sample grid; a parabola through three lags would be biased by up to a tenth of
/// a sample on this signal's narrow peak.
fn lag(a: &[f32], b: &[f32], at: usize, len: usize, near: f64) -> f64 {
    const HALF: i64 = 16;
    let corr = |l: i64| -> f64 {
        (at..at + len)
            .map(|n| f64::from(b[n]) * f64::from(a[(n as i64 + l) as usize]))
            .sum()
    };
    let c0 = near.round() as i64;
    let best = (c0 - 4..=c0 + 4)
        .max_by(|&x, &y| corr(x).total_cmp(&corr(y)))
        .expect("a range of lags");
    let c: Vec<(i64, f64)> = (best - HALF..=best + HALF).map(|k| (k, corr(k))).collect();
    let at_x = |x: f64| -> f64 {
        c.iter()
            .map(|&(k, v)| {
                let d = x - k as f64;
                let sinc = if d == 0.0 {
                    1.0
                } else {
                    (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d)
                };
                let w = 0.5 + 0.5 * (std::f64::consts::PI * d / (HALF + 1) as f64).cos();
                v * sinc * w
            })
            .sum()
    };
    (-1000..=1000)
        .map(|i| best as f64 + f64::from(i) * 1e-3)
        .max_by(|&x, &y| at_x(x).total_cmp(&at_x(y)))
        .expect("a grid")
}

/// Two devices, one on true time and one 7 ppm fast, sample one continuous signal: the
/// fast one's sample `n` is the slow one's at its own true time, so the lag between them
/// moves by 7 ppm of the elapsed samples (8.1 samples in 24 s at 48 kHz), and every sample
/// is the signal at its true capture time. The slow device hears the signal 1 ms late, so
/// the fast one's samples are found in it after the lag's whole run.
#[test]
fn two_clocks_sample_one_signal_of_true_time() {
    let blocks = (24 * RATE / BLOCK) as u64;
    const LATE_NS: i64 = 1_000_000;
    let (mut sa, mut da, oa) = open(noise_cfg(0.0, LATE_NS));
    let (mut sb, mut db, ob) = open(noise_cfg(7.0, 0));
    let (_, a) = run(&mut sa, &mut da, blocks + 4);
    let (_, b) = run(&mut sb, &mut db, blocks);
    let eps = 7e-6;
    let fs = f64::from(RATE);
    // b[n] is the signal at ob + n/(fs(1+ε)); a[m] at oa + m/fs − 1 ms.
    let expected = |n: usize| {
        (ob as f64 - oa as f64 + LATE_NS as f64) * fs / 1e9 - n as f64 * eps / (1.0 + eps)
    };
    assert!(
        expected(0) > 12.0 && expected(0) < 4.0 * f64::from(BLOCK),
        "B opened {} samples after A",
        expected(0)
    );
    let len = 8192;
    let first = lag(&a, &b, 100, len, expected(100));
    let last_at = b.len() - len - 100;
    let last = lag(&a, &b, last_at, len, expected(last_at));
    let moved = last - first;
    let want = expected(last_at) - expected(100);
    println!(
        "lag {first:.3} → {last:.3} samples (expected {:.3} → {:.3}), moved {moved:.3} of {want:.3}",
        expected(100),
        expected(last_at)
    );
    assert!(
        (first - expected(100)).abs() < 0.1,
        "{first} against {}",
        expected(100)
    );
    assert!((moved - want).abs() < 0.1, "moved {moved:.3} of {want:.3}");

    // Sample-exact against the signal itself, on both clocks.
    let mut g = TrueTimeNoise::new(RATE);
    for (x, origin, ppm, late) in [(&a, oa, 0.0, LATE_NS), (&b, ob, 7.0, 0)] {
        let worst = (0..x.len())
            .step_by(997)
            .map(|n| {
                let t = n as f64 / (fs * (1.0 + ppm * 1e-6)) * 1e9 - late as f64;
                (x[n] - reference(&mut g, origin, t)).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "{ppm} ppm: {worst} off the signal");
    }
}

/// White to 0.39 of the rate and over 60 dB down from 0.48 (a Welch average of Hann
/// windowed segments, read at single bins); −20 dBFS rms.
#[test]
fn true_time_noise_is_white_in_band_and_quiet_above_it() {
    let (mut s, mut d, _) = open(noise_cfg(0.0, 0));
    let blocks = (4 * RATE / BLOCK) as u64;
    let started = Instant::now();
    let (_, x) = run(&mut s, &mut d, blocks);
    let per_sample = started.elapsed().as_secs_f64() / x.len() as f64;
    println!(
        "{:.2} µs per frame (one shared value for both inputs) in this build",
        per_sample * 1e6
    );
    let rms = (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
    let db = 20.0 * rms.log10();
    assert!((db + 20.0).abs() < 0.3, "{db:.2} dBFS rms");

    const N: usize = 2048;
    let hann: Vec<f64> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N as f64).cos())
        .collect();
    let power = |share: f64| -> f64 {
        let bin = (share * N as f64).round();
        let w = std::f64::consts::TAU * bin / N as f64;
        let mut sum = 0.0;
        let mut segs = 0;
        for seg in x.chunks_exact(N).take(64) {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &v) in seg.iter().enumerate() {
                let y = f64::from(v) * hann[i];
                re += y * (w * i as f64).cos();
                im -= y * (w * i as f64).sin();
            }
            sum += re * re + im * im;
            segs += 1;
        }
        sum / f64::from(segs)
    };
    let shares = [0.02, 0.1, 0.2, 0.3, 0.35, 0.39];
    let band: Vec<f64> = shares.iter().map(|&f| power(f)).collect();
    let mean = band.iter().sum::<f64>() / band.len() as f64;
    for (f, p) in shares.iter().zip(&band) {
        let rel = 10.0 * (p / mean).log10();
        println!("{f:.2} × rate: {rel:+.1} dB");
        assert!(rel.abs() < 3.0, "{f} × rate: {rel:+.1} dB");
    }
    for f in [0.42, 0.45, 0.48, 0.49, 0.5] {
        let rel = 10.0 * (power(f) / mean).log10();
        println!("{f:.2} × rate: {rel:+.1} dB");
        if f >= 0.48 {
            assert!(rel < -60.0, "{f} × rate: {rel:+.1} dB");
        }
    }
}

/// The signal is a pure function of true time: two constructions (opened at different
/// wall-clock instants) give their stamps alike and each sample the signal at its own
/// true time; a delay reads the signal that much earlier.
#[test]
fn true_time_noise_is_deterministic_and_delayed_exactly() {
    let fs = f64::from(RATE);
    let mut g = TrueTimeNoise::new(RATE);
    let mut heads = Vec::new();
    for delay_ns in [0, 0, 123_456] {
        let (mut s, mut d, origin) = open(noise_cfg(3.0, delay_ns));
        let (h, x) = run(&mut s, &mut d, 20);
        let worst = (0..x.len())
            .map(|n| {
                let t = n as f64 / (fs * (1.0 + 3e-6)) * 1e9 - delay_ns as f64;
                (x[n] - reference(&mut g, origin, t)).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "delay {delay_ns}: {worst} off the signal");
        heads.push(h.iter().map(|h| h.capture_ns).collect::<Vec<_>>());
    }
    assert_eq!(heads[0], heads[1]);
}

/// A clock step at block 50: from that block's first frame the stamps advance at the new
/// rate and the samples are the signal at the new true times; before it nothing changed.
#[test]
fn a_clock_step_changes_the_samples_and_stamps_from_its_block() {
    let fs = f64::from(RATE);
    let step = FakeFault::ClockStep {
        at_block: 50,
        ppb: 5_600,
    };
    let big = FakeFault::ClockStep {
        at_block: 50,
        ppb: 20_000_000,
    };
    for (fault, ppm) in [(step, 5.6), (big, 20_000.0)] {
        let mut cfg = noise_cfg(1.0, 0);
        cfg.faults = vec![fault];
        let (mut s, mut d, origin) = open(cfg);
        let (heads, x) = run(&mut s, &mut d, 100);
        let at = 50 * BLOCK as usize;
        let t_at = at as f64 / (fs * (1.0 + 1e-6)) * 1e9;
        let true_ns = |n: usize| {
            if n < at {
                n as f64 / (fs * (1.0 + 1e-6)) * 1e9
            } else {
                t_at + (n - at) as f64 / (fs * (1.0 + (1.0 + ppm) * 1e-6)) * 1e9
            }
        };
        let mut g = TrueTimeNoise::new(RATE);
        let worst = (0..x.len())
            .map(|n| (x[n] - reference(&mut g, origin, true_ns(n))).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "{ppm} ppm step: {worst} off the signal");
        // The capture stamps are true time on an exact host crystal.
        for (b, h) in heads.iter().enumerate() {
            let want = true_ns(b * BLOCK as usize) as u64;
            let got = h.capture_ns.expect("stamped");
            assert!(got.abs_diff(want) <= 1, "block {b}: {got} against {want}");
        }
    }
}

/// An xrun's lost frames before a step count: the step starts at the first frame of its
/// block wherever the index has moved to.
#[test]
fn a_clock_step_after_an_xrun_starts_at_its_block_first_frame() {
    let fs = f64::from(RATE);
    let mut cfg = noise_cfg(0.0, 0);
    cfg.faults = vec![
        FakeFault::Xrun {
            at_block: 10,
            lost_frames: 300,
        },
        FakeFault::ClockStep {
            at_block: 20,
            ppb: 10_000_000,
        },
    ];
    let (mut s, mut d, _) = open(cfg);
    let (heads, _) = run(&mut s, &mut d, 30);
    let at = 20 * u64::from(BLOCK) + 300;
    assert_eq!(heads[20].start_sample, at);
    let t_at = at as f64 / fs * 1e9;
    for h in &heads[20..] {
        let want = t_at + (h.start_sample - at) as f64 / (fs * 1.01) * 1e9;
        let got = h.capture_ns.expect("stamped") as f64;
        assert!((got - want).abs() <= 1.0, "{got} against {want}");
    }
}

#[test]
fn blocks_without_capture_stamps() {
    let mut cfg = noise_cfg(0.0, 0);
    cfg.input_content = InputContent::Silence;
    cfg.faults = vec![FakeFault::NoCaptureStamps {
        at_block: 10,
        blocks: 5,
    }];
    let (mut s, mut d, _) = open(cfg);
    let (heads, _) = run(&mut s, &mut d, 20);
    let stamped: Vec<bool> = heads.iter().map(|h| h.capture_ns.is_some()).collect();
    let want: Vec<bool> = (0..20).map(|b| !(10..15).contains(&b)).collect();
    assert_eq!(stamped, want);
}

/// Frames delivered per second of wall-clock time while `s` runs for `secs`, after `skip`.
///
/// Each reading is the newest block of one drain and the instant the drain ended: a reader
/// that was descheduled finds a backlog, and stamping its oldest block with the late instant
/// would read as a burst of speed the device never had.
fn delivered_rate(s: &mut DuplexStream, skip: Duration, secs: f64) -> f64 {
    let mut buf = Vec::new();
    let start = Instant::now();
    while start.elapsed() < skip {
        while s.capture().pop_into(&mut buf).is_some() {}
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut first: Option<(Instant, u64)> = None;
    let mut last = None;
    while start.elapsed() < skip + Duration::from_secs_f64(secs) {
        let mut newest = None;
        while let Some(h) = s.capture().pop_into(&mut buf) {
            newest = Some(h.start_sample + u64::from(h.frames));
        }
        if let Some(end) = newest {
            let now = Instant::now();
            first.get_or_insert((now, end));
            last = Some((now, end));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let (t0, n0) = first.expect("blocks delivered");
    let (t1, n1) = last.expect("blocks delivered");
    (n1 - n0) as f64 / (t1 - t0).as_secs_f64()
}

/// In realtime pacing the device delivers at its own clock's rate: 5 % fast reads 5 % more
/// frames a wall-clock second, and a step to 5 % slower from a block on slows it.
///
/// Each rate is read over two seconds and told apart from the nominal rate by half the
/// offset: a loaded machine holding the reader or the driver for 50 ms moves a reading by
/// 2.5 %, and the driver, paced against the stream's start, makes up what it lost.
#[test]
fn slow_realtime_pacing_follows_the_device_clock() {
    let blocks_per_s = u64::from(RATE / 256);
    let backend = FakeBackend::new(FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        block_frames: 256,
        audio_clock_ppm: 50_000.0,
        // Three seconds of the device's time: 2.86 s of wall-clock time at 5 % fast.
        faults: vec![FakeFault::ClockStep {
            at_block: 3 * blocks_per_s,
            ppb: -100_000_000,
        }],
        ..FakeConfig::default()
    })
    .expect("config");
    let level = MaxLevel::from_peak_db(-6.0).expect("level");
    let mut s = backend
        .open(DuplexRequest::new(vec![0], 2, level))
        .expect("open");
    let fast = delivered_rate(&mut s, Duration::from_millis(200), 2.0) / f64::from(RATE);
    let slow = delivered_rate(&mut s, Duration::from_millis(1000), 2.0) / f64::from(RATE);
    println!("delivered at {fast:.4} then {slow:.4} of the nominal rate");
    assert!((fast - 1.05).abs() < 0.025, "fast: {fast:.4}");
    assert!((slow - 0.95).abs() < 0.025, "slow: {slow:.4}");
}
