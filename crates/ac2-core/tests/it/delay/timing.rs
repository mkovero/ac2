//! Delay finder timing (prints only).

use super::*;

/// CPU time of this thread, ms: unlike wall time it does not count waiting for a CPU on a
/// loaded machine. Wall time where the kernel does not report it.
fn cpu_ms() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    std::fs::read_to_string("/proc/thread-self/schedstat")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .map_or_else(
            || T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1e3,
            |ns| ns * 1e-6,
        )
}

#[test]
fn timing_print() {
    let mode = if cfg!(debug_assertions) {
        "debug build (ac2-core unoptimised; FFT deps at opt-level 2)"
    } else {
        "release build"
    };
    for (band, label) in [(Band::FullRange, "full"), (Band::Sub, "sub")] {
        let cfg = FinderConfig::new(FS, band); // ±1 s search, default observation
        let lm = cfg.observation_len();
        let span = cfg.search.max as usize;
        let n = lm + 2 * span;
        let (r, m) = scene(n, &[(123.4, 1.0)], -30.0, 8);
        let mut scratch = FinderScratch::for_config(&cfg).expect("config");
        let mut times = Vec::new();
        let mut last = None;
        // a second call shows the cost with plans and buffers reused; skipped in debug
        // builds to keep the test run short
        let calls = if cfg!(debug_assertions) { 1 } else { 2 };
        for _ in 0..calls {
            let t0 = cpu_ms();
            let res = find_with(
                &mut scratch,
                Block {
                    start: 0,
                    samples: &r,
                },
                Block {
                    start: span as u64,
                    samples: &m[span..span + lm],
                },
                &cfg,
            )
            .expect("config");
            times.push(cpu_ms() - t0);
            last = Some(res);
        }
        let res = last.expect("ran");
        println!(
            "timing {label}: obs {:.2} s, search ±1 s: {:.0} ms first call, {} reused \
             scratch [{mode}] -> {:?}",
            lm as f64 / FS,
            times[0],
            times
                .get(1)
                .map_or("-".to_owned(), |t| format!("{t:.0} ms")),
            status(&res),
        );
    }
    // Per-window cost of tracking through the stream (full-range band, ±1 s search): every
    // window a full search, against the tracking poll that keeps local results. Release
    // only: twenty windows of full search take too long unoptimised.
    if cfg!(debug_assertions) {
        return;
    }
    for fs in [48_000.0, 96_000.0] {
        let cfg = FinderConfig::new(fs, Band::FullRange);
        let lm = cfg.observation_len();
        let span = cfg.search.max as usize;
        let n_win = 20;
        let n = 2 * span + n_win * lm + 1024;
        let (r, m) = scene(n, &[(fs * 0.004 + 0.4, 1.0), (fs * 0.009, -0.5)], -30.0, 12);
        let block = (fs / 48_000.0 * 256.0) as usize;
        let run = |tracking: bool| {
            let mut s = DelayStream::new(cfg).expect("config");
            let mut t = Tracker::new(Agreement::for_band(Band::FullRange, fs));
            let mut times = Vec::new();
            for (i, (rc, mc)) in r.chunks(block).zip(m.chunks(block)).enumerate() {
                let start = (i * block) as u64;
                s.push_ref(Block { start, samples: rc });
                s.push_meas(Block { start, samples: mc });
                loop {
                    let t0 = cpu_ms();
                    let res = if tracking {
                        s.poll_tracking(t.held())
                    } else {
                        s.poll()
                    };
                    let Some(res) = res.expect("config") else {
                        break;
                    };
                    times.push(cpu_ms() - t0);
                    t.observe(&res);
                }
            }
            let mean = times.iter().sum::<f64>() / times.len() as f64;
            let min = times.iter().copied().fold(f64::INFINITY, f64::min);
            (times.len(), mean, min, s.full_searches(), t.held())
        };
        let (nf, mean_f, min_f, _, held_f) = run(false);
        let (nt, mean_t, min_t, full_t, held_t) = run(true);
        assert_eq!(held_f, held_t, "tracking holds what the full search holds");
        println!(
            "timing tracking at {} kHz (thread CPU time): full search every window {mean_f:.1} ms/window \
             (min {min_f:.1}, {nf} windows); tracking poll {mean_t:.1} ms/window (min \
             {min_t:.1}, {full_t} of {nt} windows full) -> {:.1}x; window = {:.0} ms of audio",
            fs / 1e3,
            mean_f / mean_t,
            lm as f64 / fs * 1e3,
        );
    }
}
