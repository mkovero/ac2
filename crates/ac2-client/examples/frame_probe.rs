//! Diagnostic: log frame arrival gaps, age and STALE per topic from a local daemon.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig};
use ac2_proto::Subscription;

#[tokio::main]
async fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let c = Client::connect(ClientConfig::local("frame-probe"))
        .await
        .expect("connect");
    c.subscribe(Subscription::AllData).expect("subscribe");
    c.subscribe(Subscription::InputMeters).expect("subscribe");
    let start = Instant::now();
    let mut last_seq: BTreeMap<String, (u64, Instant)> = BTreeMap::new();
    let mut max_gap: BTreeMap<String, Duration> = BTreeMap::new();
    let mut stale_count: BTreeMap<String, u32> = BTreeMap::new();
    let mut frames: BTreeMap<String, u32> = BTreeMap::new();
    while start.elapsed() < Duration::from_secs(secs) {
        let l = c.latest().expect("latest");
        for (t, f) in &l.frames {
            let seq = f.frame.stamp.seq;
            let e = last_seq.entry(t.to_string()).or_insert((seq, f.received));
            if seq != e.0 {
                let gap = f.received.duration_since(e.1);
                if gap > Duration::from_millis(250) {
                    println!(
                        "{:7.3}s {t}: gap {:?} (seq {}→{}) age {:?}",
                        start.elapsed().as_secs_f64(),
                        gap,
                        e.0,
                        seq,
                        f.age
                    );
                }
                let m = max_gap.entry(t.to_string()).or_default();
                if gap > *m {
                    *m = gap;
                }
                *frames.entry(t.to_string()).or_default() += 1;
                *e = (seq, f.received);
            }
            if f.stale {
                *stale_count.entry(t.to_string()).or_default() += 1;
                println!(
                    "{:7.3}s {t}: STALE since_new {:?} age {:?} responding {}",
                    start.elapsed().as_secs_f64(),
                    f.since_new,
                    f.age,
                    l.responding
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    println!("--- summary over {secs}s");
    for (t, n) in &frames {
        println!(
            "{t}: {n} new frames ({:.1}/s), max gap {:?}, stale polls {}",
            *n as f64 / secs as f64,
            max_gap[t],
            stale_count.get(t).copied().unwrap_or(0)
        );
    }
}
