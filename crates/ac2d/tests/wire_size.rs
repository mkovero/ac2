//! What each live stream costs on the wire: bytes per frame and frames per second of every
//! stream of a transfer function, a default spectrum, an RTA and an SPL meter on the fake
//! rig, and the daemon's CPU time over the run. The spectrum frame must stay display-sized:
//! a 65 536-point FFT is 32 769 bins, far more than any screen has pixels. A realistic
//! tuning session (three transfer functions and their average, three default spectra and a
//! short one, an RTA, two SPL meters with band meters, the input meters) must fit a remote
//! client's budget at 60 frames per second.
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
use ac2_proto::model::{
    BandFraction, BandLeqPreset, GeneratorDesired, GeneratorSettings, MathConfig, MathDomain,
    MeasConfig, MeasKind, Operand, RtaConfig, SessionConfig, Signal, SpectrumConfig, TraceOwner,
};
use ac2_proto::units::{Dbfs, MeasId};
use ac2_proto::{Command, DataMessage, ReplyBody, decode_data_message};
use ac2d::{Daemon, DaemonConfig};
use common::*;

/// A direct sound and one reflection `lag` samples later.
fn comb(lag: usize) -> Vec<f32> {
    let mut fir = vec![0.0; lag + 1];
    fir[0] = 0.7;
    fir[lag] = 0.3;
    fir
}

fn noisy_rig() -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Thread(Pace::Realtime),
        seed: 7,
        input_noise_rms: 1e-2,
        // The measurement inputs arrive with the reference, as after `delay.insert`, through
        // short combs (a reflection), so the transfer functions have the shape and the phase
        // of a delay-compensated loudspeaker; the input noise makes the high bands noisy.
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY, comb(9), 0.0),
            FakePath::acoustic(0, 2, LOOP_DELAY + 1, comb(23), 0.0),
            FakePath::acoustic(0, 3, LOOP_DELAY + 2, comb(61), 0.0),
        ],
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

/// Per topic: frames and bytes (every part, as the subscriber receives them).
type Sizes = BTreeMap<String, (u64, u64)>;

/// What one run sets up and receives.
struct Setup<'a> {
    meas: &'a [MeasConfig],
    /// Session inputs.
    inputs: Vec<u16>,
    /// Pink noise out of output 0 (into every input on the fake rig).
    stimulus: bool,
    /// The subscriber's prefixes.
    prefixes: Vec<Vec<u8>>,
    fps: Option<u32>,
}

fn run(meas: &[MeasConfig], seconds: f64) -> Sizes {
    run_with(
        &Setup {
            meas,
            inputs: vec![0, 1],
            stimulus: false,
            prefixes: vec![b"d/".to_vec(), b"ka".to_vec()],
            fps: None,
        },
        seconds,
    )
}

fn run_with(setup: &Setup<'_>, seconds: f64) -> Sizes {
    init_log();
    let mut config = DaemonConfig::new(Arc::new(noisy_rig()), local_tcp(), -10.0);
    if let Some(fps) = setup.fps {
        config.publish_fps = fps;
    }
    config.lease_expiry = Duration::from_secs(600);
    let h = Daemon::start(config).unwrap();
    let prefixes: Vec<&[u8]> = setup.prefixes.iter().map(Vec::as_slice).collect();
    let (mut c, sub) = connect(&h, &prefixes);
    c.ok(Command::SessionOpen {
        config: SessionConfig {
            input_channels: setup.inputs.clone(),
            ..session(false)
        },
    });
    for (i, m) in setup.meas.iter().enumerate() {
        c.ok(Command::MeasCreate { config: m.clone() });
        c.ok(Command::MeasStart {
            meas: MeasId(i as u32 + 1),
        });
    }
    if setup.stimulus {
        let lease_token = match c.ok(Command::GenAcquire { force: false }) {
            ReplyBody::Lease(l) => l.lease_token,
            other => panic!("{other:?}"),
        };
        c.ok(Command::GenSet {
            lease_token,
            desired: GeneratorDesired {
                settings: GeneratorSettings {
                    signal: Signal::Pink,
                    level: Dbfs(-20.0),
                    band: None,
                    outputs: vec![0],
                },
                armed: true,
                firing: true,
            },
        });
    }
    // Let the averages fill before counting.
    std::thread::sleep(Duration::from_secs(2));
    while sub.sock.recv_timeout(Duration::ZERO).unwrap().is_some() {}
    let mut sizes = Sizes::new();
    // Every frame's parts, `[u32 count, (u32 len, bytes)…]` little-endian, appended to one
    // file per topic, for trying encodings offline.
    let dump = std::env::var_os("AC2_WIRE_DUMP").map(std::path::PathBuf::from);
    let (t0, c0) = (Instant::now(), cpu_s());
    while t0.elapsed() < Duration::from_secs_f64(seconds) {
        let Some(m) = sub.sock.recv_timeout(Duration::from_millis(100)).unwrap() else {
            continue;
        };
        let parts: Vec<&[u8]> = m.frames().iter().map(Vec::as_slice).collect();
        if let Ok(DataMessage::Frame(_)) = decode_data_message(&parts) {
            let topic = String::from_utf8_lossy(parts[0]).into_owned();
            let bytes: usize = parts.iter().map(|p| p.len()).sum();
            if let Some(dir) = &dump {
                use std::io::Write;
                let name = format!("{}.bin", topic.replace('/', "_"));
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join(name))
                    .unwrap();
                f.write_all(&(parts.len() as u32).to_le_bytes()).unwrap();
                for p in &parts {
                    f.write_all(&(p.len() as u32).to_le_bytes()).unwrap();
                    f.write_all(p).unwrap();
                }
            }
            let e = sizes.entry(topic).or_default();
            e.0 += 1;
            e.1 += bytes as u64;
        }
    }
    let (c1, elapsed) = (cpu_s(), t0.elapsed().as_secs_f64());
    let (cpu, jobs) = ((c1.0 - c0.0) / elapsed, (c1.1 - c0.1) / elapsed);
    let (mut frames, mut bytes) = (0, 0);
    for (topic, (n, b)) in &sizes {
        eprintln!(
            "{topic:>16}: {:>8.0} B/frame {:>5.1} frames/s {:>9.0} B/s",
            *b as f64 / *n as f64,
            *n as f64 / seconds,
            *b as f64 / seconds
        );
        frames += n;
        bytes += b;
    }
    eprintln!(
        "{:>16}: {:>8.0} B/frame {:>5.1} frames/s {:>9.0} B/s = {:.2} Mbit/s (payload)",
        "total",
        bytes as f64 / frames.max(1) as f64,
        frames as f64 / seconds,
        bytes as f64 / seconds,
        bytes as f64 * 8.0 / seconds / 1e6,
    );
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

fn short_spectrum(name: &str) -> MeasConfig {
    let mut config = SpectrumConfig::on_input(2);
    config.fft_len = 4096;
    MeasConfig {
        name: name.into(),
        kind: MeasKind::Spectrum { config },
    }
}

fn banded_spl(name: &str, input: u16) -> MeasConfig {
    let mut m = spl(name, input);
    if let MeasKind::Spl { config } = &mut m.kind {
        config.bands = Some(Box::new(BandLeqPreset::Finland545Lf.apply(None)));
    }
    m
}

/// The session a remote client follows while tuning: measurement ids in creation order.
fn tuning_session() -> Vec<MeasConfig> {
    let tf = |name: &str, input: u16| {
        let mut m = transfer(name);
        if let MeasKind::Transfer { config } = &mut m.kind {
            config.measurement_input = input;
        }
        m
    };
    let spec_on = |name: &str, input: u16| MeasConfig {
        name: name.into(),
        kind: MeasKind::Spectrum {
            config: SpectrumConfig::on_input(input),
        },
    };
    vec![
        tf("main L", 1),
        tf("main R", 2),
        tf("sub", 3),
        MeasConfig {
            name: "avg".into(),
            kind: MeasKind::Math {
                config: MathConfig::power_average(
                    TraceOwner::Imported,
                    MathDomain::Transfer,
                    (1..=3).map(|m| Operand::Meas { meas: MeasId(m) }).collect(),
                ),
            },
        },
        spec_on("spec 1", 1),
        spec_on("spec 2", 2),
        spec_on("spec 3", 3),
        short_spectrum("spec short"),
        MeasConfig {
            name: "rta".into(),
            kind: MeasKind::Rta {
                config: RtaConfig::on_input(1, BandFraction::Third),
            },
        },
        banded_spl("spl FOH", 1),
        banded_spl("spl stage", 2),
    ]
}

/// What the app subscribes to with the transfer and spectrum panes in view: no IR, no
/// per-measurement meters.
fn ui_topics() -> Vec<Vec<u8>> {
    let mut t: Vec<String> = (1..=4).map(|m| format!("d/{m}/tf")).collect();
    t.extend((5..=8).map(|m| format!("d/{m}/spec")));
    t.push("d/9/rta".into());
    for m in [10, 11] {
        t.extend(["spl", "leq", "band_leq"].map(|s| format!("d/{m}/{s}")));
    }
    t.push("session/levels".into());
    t.into_iter().map(String::into_bytes).collect()
}

fn session_sizes(fps: u32, seconds: f64) -> Sizes {
    let meas = tuning_session();
    run_with(
        &Setup {
            meas: &meas,
            inputs: vec![0, 1, 2, 3],
            stimulus: true,
            prefixes: ui_topics(),
            fps: Some(fps),
        },
        seconds,
    )
}

/// The tuning session at 60 frames per second stays far below what a Wi-Fi or VPN link
/// carries (about 3 Mbit/s on the fake rig, where noisy high bands make the transfer
/// functions dear; it was 9.3 with f32 arrays).
#[test]
fn slow_tuning_session_fits_a_remote_link() {
    let seconds = 4.0;
    let sizes = session_sizes(60, seconds);
    let bits_per_s = sizes.values().map(|(_, b)| *b as f64).sum::<f64>() * 8.0 / seconds;
    assert!(bits_per_s < 4.5e6, "{:.2} Mbit/s", bits_per_s / 1e6);
    for (topic, (n, b)) in &sizes {
        let per_frame = b / n;
        if topic.ends_with("/tf") {
            assert!(per_frame < 3_000, "{topic}: {per_frame} B");
        }
        if topic.ends_with("/spec") {
            assert!(per_frame < 2_000, "{topic}: {per_frame} B");
        }
    }
}

/// Bytes a remote client receives in a tuning session at 30 and 60 frames per second,
/// printed (`--ignored --nocapture`).
#[test]
#[ignore = "measurement, not a check"]
fn tuning_session_bytes() {
    for fps in [30, 60] {
        eprintln!("--- {fps} fps");
        session_sizes(fps, 10.0);
    }
}
