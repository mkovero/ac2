//! What each live stream costs on the wire: bytes per frame and frames per second of every
//! stream of a transfer function, a default spectrum, an RTA and an SPL meter on the fake
//! rig, and the daemon's CPU time over the run. The spectrum frame must stay display-sized:
//! a 65 536-point FFT is 32 769 bins, far more than any screen has pixels.
//!
//! Its own binary: the CPU time it reports is the whole process's.
#![allow(clippy::unwrap_used)]

#[path = "it/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath, Pace};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_proto::model::{BandFraction, MeasConfig, MeasKind, RtaConfig, SpectrumConfig};
use ac2_proto::units::MeasId;
use ac2_proto::{Command, DataMessage, decode_data_message};
use ac2d::{Daemon, DaemonConfig};
use common::*;

fn noisy_rig() -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Thread(Pace::Realtime),
        seed: 7,
        input_noise_rms: 1e-2,
        paths: vec![FakePath::loopback(0, 0, LOOP_DELAY)],
        ..FakeConfig::default()
    })
    .unwrap()
}

/// CPU time (user + system) of a `/proc` stat file, seconds.
fn stat_cpu_s(path: &std::path::Path) -> f64 {
    let stat = std::fs::read_to_string(path).unwrap_or_default();
    let after = stat.rsplit(')').next().unwrap_or("");
    let f: Vec<&str> = after.split_whitespace().collect();
    let ticks = |i: usize| f.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    // Fields after the comm: state is index 0, utime 11, stime 12; 100 ticks/s on Linux.
    (ticks(11) + ticks(12)) / 100.0
}

/// Process CPU time, and that of the measurement job threads, seconds.
fn cpu_s() -> (f64, f64) {
    let process = stat_cpu_s(std::path::Path::new("/proc/self/stat"));
    let mut jobs = 0.0;
    for t in std::fs::read_dir("/proc/self/task")
        .into_iter()
        .flatten()
        .flatten()
    {
        let comm = std::fs::read_to_string(t.path().join("comm")).unwrap_or_default();
        if comm.starts_with("ac2d-meas-") {
            jobs += stat_cpu_s(&t.path().join("stat"));
        }
    }
    (process, jobs)
}

fn run(meas: &[MeasConfig], seconds: f64) -> BTreeMap<String, (u64, u64)> {
    init_log();
    let h = Daemon::start(DaemonConfig::new(Arc::new(noisy_rig()), local_tcp(), -10.0)).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/", b"ka"]);
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    for (i, m) in meas.iter().enumerate() {
        c.ok(Command::MeasCreate { config: m.clone() });
        c.ok(Command::MeasStart {
            meas: MeasId(i as u32 + 1),
        });
    }
    // Let the averages fill before counting.
    std::thread::sleep(Duration::from_secs(2));
    while sub.sock.recv_timeout(Duration::ZERO).unwrap().is_some() {}
    let mut sizes: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let (t0, c0) = (Instant::now(), cpu_s());
    while t0.elapsed() < Duration::from_secs_f64(seconds) {
        let Some(m) = sub.sock.recv_timeout(Duration::from_millis(100)).unwrap() else {
            continue;
        };
        let parts: Vec<&[u8]> = m.frames().iter().map(Vec::as_slice).collect();
        if let Ok(DataMessage::Frame(_)) = decode_data_message(&parts) {
            let topic = String::from_utf8_lossy(parts[0]).into_owned();
            let bytes: usize = parts.iter().map(|p| p.len()).sum();
            let e = sizes.entry(topic).or_default();
            e.0 += 1;
            e.1 += bytes as u64;
        }
    }
    let (c1, elapsed) = (cpu_s(), t0.elapsed().as_secs_f64());
    let (cpu, jobs) = ((c1.0 - c0.0) / elapsed, (c1.1 - c0.1) / elapsed);
    for (topic, (n, b)) in &sizes {
        eprintln!(
            "{topic:>14}: {:>8.0} B/frame {:>5.1} frames/s {:>9.0} B/s",
            *b as f64 / *n as f64,
            *n as f64 / seconds,
            *b as f64 / seconds
        );
    }
    eprintln!(
        "cpu {:.1} % of one core (measurement jobs {:.1} %)",
        cpu * 100.0,
        jobs * 100.0
    );
    sizes
}

fn spectrum(name: &str) -> MeasConfig {
    MeasConfig {
        name: name.into(),
        kind: MeasKind::Spectrum {
            config: SpectrumConfig::on_input(1),
        },
    }
}

#[test]
fn slow_live_streams_stay_display_sized() {
    let sizes = run(
        &[
            transfer("tf"),
            spectrum("spec"),
            MeasConfig {
                name: "rta".into(),
                kind: MeasKind::Rta {
                    config: RtaConfig::on_input(1, BandFraction::Third),
                },
            },
            spl("spl", 1),
        ],
        3.0,
    );
    let (n, b) = sizes["d/2/spec"];
    let per_frame = b / n;
    assert!(per_frame < 16_000, "spectrum frame {per_frame} B");
}

/// Daemon CPU for one default spectrum alone, printed (`--ignored --nocapture`).
#[test]
#[ignore = "measurement, not a check"]
fn spectrum_cpu() {
    run(&[spectrum("spec")], 10.0);
}
