//! Throughput / CPU sanity check for the ac2 data path.
//!
//! `cargo run --release -p spike-zmq-curve --example throughput`
//!
//! 1. Paced: 8 topics x 60 fps x 6 KB frames over tcp://127.0.0.1 for a few seconds, with
//!    and without CURVE; reports process CPU (publisher + subscriber + libzmq I/O threads)
//!    and publish→receive latency.
//! 2. Burst: as fast as possible, to see the headroom.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use spike_zmq_curve::ctrl::{ClientSecurity, ServerSecurity, apply_client};
use spike_zmq_curve::data::{SubEvent, bind_xpub};
use spike_zmq_curve::ffi;
use spike_zmq_curve::proto::{FrameHeader, FrameKind, decode_frame, encode_frame};
use spike_zmq_curve::zap::{AuthorizedClients, ZapHandler};
use spike_zmq_curve::zmq::{Context, CurveKeyPair, SocketType, z85_decode_key};

type R<T> = Result<T, Box<dyn std::error::Error>>;

const TOPICS: usize = 8;
const FPS: u64 = 60;
const N: u32 = 1536; // 6144-byte payload
const PACED_SECS: u64 = 5;

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

#[cfg(unix)]
fn cpu_time() -> Duration {
    // SAFETY: getrusage fills a zeroed struct.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: valid out-pointer.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| {
        Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64)
    };
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

#[cfg(not(unix))]
fn cpu_time() -> Duration {
    Duration::ZERO
}

struct Setup {
    ctx: Context,
    server: ServerSecurity,
    client: ClientSecurity,
    _zap: Option<ZapHandler>,
}

fn setup(curve: bool) -> R<Setup> {
    let ctx = Context::new()?;
    if !curve {
        return Ok(Setup {
            ctx,
            server: ServerSecurity::Null,
            client: ClientSecurity::Null,
            _zap: None,
        });
    }
    let s = CurveKeyPair::generate()?;
    let c = CurveKeyPair::generate()?;
    let mut list = AuthorizedClients::new();
    list.insert(z85_decode_key(&c.public)?, "bench".into());
    let zap = ZapHandler::start(&ctx, "ac2", list)?;
    Ok(Setup {
        server: ServerSecurity::Curve {
            keys: s.clone(),
            zap_domain: "ac2".into(),
        },
        client: ClientSecurity::Curve {
            keys: c,
            server_public: s.public,
        },
        ctx,
        _zap: Some(zap),
    })
}

/// Runs a subscriber thread; returns (frames received, latencies in µs).
fn run(
    curve: bool,
    frames_per_topic: u64,
    pace: Option<Duration>,
) -> R<(u64, Vec<u64>, Duration, Duration)> {
    let st = setup(curve)?;
    // Burst: unlimited HWM so the number measures transport speed, not HWM drops.
    let hwm = if pace.is_some() { 1000 } else { 0 };
    let xpub = bind_xpub(&st.ctx, "tcp://127.0.0.1:*", &st.server, hwm)?;
    let ep = xpub.last_endpoint()?;
    let sub = st.ctx.socket(SocketType::Sub)?;
    sub.set_int(ffi::ZMQ_RCVHWM, 100_000)?;
    apply_client(&sub, &st.client)?;
    sub.subscribe(b"d/")?;
    sub.connect(&ep)?;
    let m = xpub
        .recv_timeout(Duration::from_secs(10))?
        .ok_or("no subscription")?;
    assert!(matches!(SubEvent::parse(&m), Some(SubEvent::Subscribe(_))));

    let done = Arc::new(AtomicBool::new(false));
    let done2 = done.clone();
    let expected = frames_per_topic * TOPICS as u64;
    let rx = std::thread::spawn(move || {
        let mut got = 0u64;
        let mut lat = Vec::with_capacity(expected as usize);
        while got < expected && !done2.load(Ordering::Relaxed) {
            let Ok(Some(m)) = sub.recv_timeout(Duration::from_millis(200)) else {
                continue;
            };
            if let Ok(f) = decode_frame(&m.frames) {
                lat.push(now_ns().saturating_sub(f.header.capture_wall_ns) / 1000);
                got += 1;
            }
        }
        (got, lat)
    });

    let topics: Vec<String> = (0..TOPICS).map(|i| format!("d/m{i}/tf")).collect();
    let values: Vec<f32> = (0..N).map(|i| i as f32).collect();
    let cpu0 = cpu_time();
    let t0 = Instant::now();
    for seq in 0..frames_per_topic {
        for t in &topics {
            let h = FrameHeader {
                seq,
                audio_sample: seq * 800,
                daemon_incarnation: 1,
                config_rev: 1,
                grid_id: 1,
                kind: FrameKind::Tf,
                n: N,
                capture_wall_ns: now_ns(),
            };
            xpub.send_multipart(&encode_frame(t, &h, &values), 0)?;
        }
        if let Some(p) = pace {
            let next = t0 + p * u32::try_from(seq + 1)?;
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !rx.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    done.store(true, Ordering::Relaxed);
    let (got, mut lat) = rx.join().map_err(|_| "rx panicked")?;
    let wall = t0.elapsed();
    let cpu = cpu_time() - cpu0;
    lat.sort_unstable();
    Ok((got, lat, wall, cpu))
}

fn pct(v: &[u64], p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v[((v.len() - 1) as f64 * p) as usize]
}

fn main() -> R<()> {
    for curve in [false, true] {
        let label = if curve { "CURVE" } else { "NULL " };
        let (got, lat, wall, cpu) = run(
            curve,
            FPS * PACED_SECS,
            Some(Duration::from_nanos(1_000_000_000 / FPS)),
        )?;
        println!(
            "paced {label}: {TOPICS} topics x {FPS} fps x 6 KB, {:.1} s: {got} frames, \
             CPU {:.1}% of one core, latency p50 {} µs p99 {} µs max {} µs",
            wall.as_secs_f64(),
            100.0 * cpu.as_secs_f64() / wall.as_secs_f64(),
            pct(&lat, 0.5),
            pct(&lat, 0.99),
            lat.last().copied().unwrap_or(0),
        );
    }
    for curve in [false, true] {
        let label = if curve { "CURVE" } else { "NULL " };
        let per_topic = 5000;
        let (got, _lat, wall, cpu) = run(curve, per_topic, None)?;
        let mb = got as f64 * 6.2e-3;
        println!(
            "burst {label}: {got} frames in {:.2} s = {:.0} frames/s, {:.0} MB/s, CPU {:.0}%",
            wall.as_secs_f64(),
            got as f64 / wall.as_secs_f64(),
            mb / wall.as_secs_f64(),
            100.0 * cpu.as_secs_f64() / wall.as_secs_f64(),
        );
    }
    Ok(())
}
