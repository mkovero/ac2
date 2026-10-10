//! Diagnostic: per-topic frame arrival timing, local or remote.
//!
//! ```text
//! frame_probe [--remote HOST[:PORT]] [--key-dir DIR] [--secs N] [--meas ID]... [--gap MS]
//!             [--poll MS]
//! ```
//!
//! Without `--meas` it subscribes to every measurement stream and the input meters. Per
//! topic it reports, over the run: frames seen, the receive-interval histogram (10 ms
//! buckets), the daemon-side cadence (`capture_wall_ns` deltas) and `audio_sample` deltas,
//! seq skips (frames never seen: coalesced in this client between drains, or by the daemon),
//! and the age at arrival (capture to receipt, through the keepalive clock offset). Each
//! receive interval over `--gap` ms is printed as it happens with its capture interval, so
//! a hold that the daemon made is told apart from one the transport made.
//!
//! By default it drains on every queued message. `--poll MS` drains on a fixed period
//! instead, as a display link does, so the receive intervals are what such a consumer sees.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, Endpoints, KeyDir, Latest, RemoteAddr};
use ac2_proto::Subscription;
use ac2_proto::units::MeasId;

const BUCKET_MS: f64 = 10.0;

#[derive(Default)]
struct Hist {
    buckets: BTreeMap<u64, u32>,
    vals: Vec<f64>,
}

impl Hist {
    fn add(&mut self, ms: f64) {
        *self
            .buckets
            .entry((ms.max(0.0) / BUCKET_MS) as u64)
            .or_default() += 1;
        self.vals.push(ms);
    }

    fn summary(&mut self) -> String {
        if self.vals.is_empty() {
            return "-".into();
        }
        self.vals.sort_by(f64::total_cmp);
        let q = |p: f64| self.vals[((self.vals.len() - 1) as f64 * p).round() as usize];
        let mean = self.vals.iter().sum::<f64>() / self.vals.len() as f64;
        let hist: Vec<String> = self
            .buckets
            .iter()
            .map(|(b, n)| format!("{}:{n}", b * BUCKET_MS as u64))
            .collect();
        format!(
            "n {} mean {mean:.1} min {:.1} p50 {:.1} p95 {:.1} max {:.1} ms\n      [{}]",
            self.vals.len(),
            q(0.0),
            q(0.5),
            q(0.95),
            q(1.0),
            hist.join(" ")
        )
    }
}

#[derive(Default)]
struct TopicStats {
    /// seq, received, capture_wall_ns, audio_sample of the previous new frame.
    last: Option<(u64, Instant, u64, u64)>,
    frames: u32,
    skipped: u64,
    recv: Hist,
    capture: Hist,
    age: Hist,
    samples: BTreeMap<u64, u32>,
}

struct Args {
    remote: Option<RemoteAddr>,
    key_dir: Option<PathBuf>,
    secs: u64,
    meas: Vec<u32>,
    gap_ms: f64,
    poll: Option<Duration>,
}

fn args() -> Args {
    let mut a = Args {
        remote: None,
        key_dir: None,
        secs: 10,
        meas: Vec::new(),
        gap_ms: 120.0,
        poll: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_else(|| panic!("{k} needs a value"));
        match k.as_str() {
            "--remote" => a.remote = Some(v.parse().expect("--remote")),
            "--key-dir" => a.key_dir = Some(v.into()),
            "--secs" => a.secs = v.parse().expect("--secs"),
            "--meas" => a.meas.push(v.parse().expect("--meas")),
            "--gap" => a.gap_ms = v.parse().expect("--gap"),
            "--poll" => {
                a.poll = Some(Duration::from_secs_f64(
                    v.parse::<f64>().expect("--poll") / 1e3,
                ));
            }
            _ => panic!("unknown argument {k}"),
        }
    }
    a
}

#[tokio::main]
async fn main() {
    let a = args();
    let cfg = match &a.remote {
        Some(r) => {
            let keys = KeyDir::new(
                a.key_dir
                    .clone()
                    .unwrap_or_else(ac2_client::keys::default_key_dir),
            );
            let mut cfg = ClientConfig::new(Endpoints::remote(r), "frame-probe");
            cfg.curve = Some(keys.curve_client(&r.host).expect("pinned key"));
            cfg
        }
        None => ClientConfig::local("frame-probe"),
    };
    let c = Client::connect(cfg).await.expect("connect");
    if a.meas.is_empty() {
        c.subscribe(Subscription::AllData).expect("subscribe");
        c.subscribe(Subscription::InputMeters).expect("subscribe");
    } else {
        for m in &a.meas {
            c.subscribe(Subscription::Meas(MeasId(*m)))
                .expect("subscribe");
        }
    }
    let start = Instant::now();
    let end = Duration::from_secs(a.secs);
    let mut latest = Latest::default();
    let mut topics: BTreeMap<String, TopicStats> = BTreeMap::new();
    let (mut drains, mut multi) = (0u64, 0u64);
    let mut next_poll = tokio::time::Instant::now();
    while start.elapsed() < end {
        match a.poll {
            Some(p) => {
                next_poll += p;
                tokio::time::sleep_until(next_poll).await;
            }
            // Wake on each queued message, so `received` is the arrival time to within a
            // scheduler wake-up and the client mailbox rarely holds two frames of one topic.
            None => {
                let _ = tokio::time::timeout(Duration::from_millis(50), c.data_changed()).await;
            }
        }
        c.latest_into(&mut latest).expect("latest");
        drains += 1;
        if latest.read > 1 {
            multi += 1;
        }
        for (t, f) in &latest.frames {
            let s = &f.frame.stamp;
            let (seq, cap, smp) = (s.seq, s.capture_wall_ns.0, s.audio_sample.0);
            let e = topics.entry(t.to_string()).or_default();
            if let Some((ls, lr, lc, lsm)) = e.last {
                if ls == seq {
                    continue;
                }
                let recv = f.received.duration_since(lr).as_secs_f64() * 1e3;
                let capt = (cap as f64 - lc as f64) / 1e6;
                e.recv.add(recv);
                e.capture.add(capt);
                *e.samples.entry(smp.wrapping_sub(lsm)).or_default() += 1;
                e.skipped += seq.saturating_sub(ls + 1);
                if recv > a.gap_ms {
                    println!(
                        "{:7.3}s {t}: recv gap {recv:.1} ms, capture gap {capt:.1} ms, seq {ls}->{seq}, age {}",
                        start.elapsed().as_secs_f64(),
                        f.age.map_or("-".into(), |x| format!("{:.1} ms", x * 1e3)),
                    );
                }
            }
            if let Some(age) = f.age {
                e.age.add(age * 1e3);
            }
            e.frames += 1;
            e.last = Some((seq, f.received, cap, smp));
        }
    }
    println!(
        "--- summary over {}s: {drains} drains, {multi} read >1 message",
        a.secs
    );
    for (t, e) in &mut topics {
        println!(
            "{t}: {} frames ({:.2}/s), seq skipped {}",
            e.frames,
            f64::from(e.frames) / a.secs as f64,
            e.skipped
        );
        println!("  recv interval    {}", e.recv.summary());
        println!("  capture interval {}", e.capture.summary());
        println!("  age at arrival   {}", e.age.summary());
        let s: Vec<String> = e.samples.iter().map(|(d, n)| format!("{d}:{n}")).collect();
        println!("  audio_sample deltas [{}]", s.join(" "));
    }
}
