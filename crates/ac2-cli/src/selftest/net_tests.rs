use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::thread;

use super::stats::quantiles;
use super::*;

const RATE: u32 = 96_000;
/// 1500 blocks a second: two seconds stay short and still carry thousands of blocks.
const FRAMES: u32 = 64;

fn provenance(host: &str) -> Provenance {
    Provenance {
        host: host.into(),
        ac2_build: "test".into(),
    }
}

fn spec(role: Role, seconds: f64, credit_pause: Option<(Duration, Duration)>) -> NetSpec {
    NetSpec {
        role,
        rate: RATE,
        frames: FRAMES,
        channels: 1,
        duration: Duration::from_secs_f64(seconds),
        window: WINDOW,
        credit_pause,
    }
}

/// Runs a listener and a connector on loopback; returns (listener's, connector's) results.
fn pair(
    dir: &Path,
    listener: NetSpec,
    connector: NetSpec,
) -> (Result<NetMeta, NetError>, Result<NetMeta, NetError>) {
    let ep = listen("127.0.0.1:0").expect("listen");
    let key = *ep.server_key();
    let addr = ep.addr().trim_start_matches("tcp://").to_owned();
    let stop = AtomicBool::new(false);
    let (ld, cd) = (dir.join("listener"), dir.join("connector"));
    thread::scope(|s| {
        let l = s.spawn(|| run(ep, &listener, &ld, provenance("l"), &stop));
        let c = s.spawn(|| {
            let ep = connect(&addr, key).expect("connect");
            run(ep, &connector, &cd, provenance("c"), &stop)
        });
        (
            l.join().expect("listener thread"),
            c.join().expect("connector thread"),
        )
    })
}

fn expected_blocks(seconds: f64) -> u64 {
    (seconds * f64::from(RATE) / f64::from(FRAMES)).ceil() as u64
}

#[test]
fn a_loopback_run_counts_every_block_and_summarises() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (l, c) = pair(
        tmp.path(),
        spec(Role::Receive, 1.5, None),
        spec(Role::Send, 1.5, None),
    );
    let (rx, tx) = (l.expect("receiver"), c.expect("sender"));
    let sent = tx.sent.expect("sender's counts");
    let got = rx.received.expect("receiver's counts");
    assert_eq!(sent.blocks, expected_blocks(1.5));
    assert!(!sent.interrupted && !sent.peer_gone);
    assert_eq!(got.ended, Ended::Sender);
    assert_eq!(got.received, sent.blocks);
    assert_eq!((got.missing, got.reordered), (0, 0));
    assert!(got.gaps.is_empty());
    assert_eq!(got.sender.as_ref().map(|s| s.blocks), Some(sent.blocks));
    let sum = got.summary.expect("summary");
    assert_eq!(sum.blocks, sent.blocks);
    assert!(sum.one_way.p50_us <= sum.one_way.p99_us && sum.one_way.p99_us <= sum.one_way.max_us);
    assert!(
        (sum.block_period_us - 666.666).abs() < 0.01,
        "{}",
        sum.block_period_us
    );
    assert!(sum.link_offset_ms * 1e3 >= sum.block_period_us);
    assert_eq!((rx.peer_host.as_str(), rx.side), ("c", Side::Listen));

    let bin = fs::read(tmp.path().join("listener/arrivals.bin")).expect("arrivals");
    assert_eq!(&bin[..8], MAGIC);
    assert_eq!(
        bin.len(),
        HEADER_BYTES + RECORD_BYTES * sent.blocks as usize
    );
    let last = &bin[bin.len() - RECORD_BYTES..];
    let seq = u64::from_le_bytes(last[..8].try_into().expect("8 bytes"));
    assert_eq!(seq, sent.blocks - 1);
    let meta: serde_json::Value =
        serde_json::from_slice(&fs::read(tmp.path().join("listener/meta.json")).expect("meta"))
            .expect("json");
    assert_eq!(meta["received"]["received"], sent.blocks);
    assert_eq!(meta["role"], "receive");
    assert!(tmp.path().join("connector/meta.json").is_file());
    assert!(!tmp.path().join("connector/arrivals.bin").exists());
}

#[test]
fn a_stalled_receiver_makes_late_sends_not_drops() {
    let tmp = tempfile::tempdir().expect("tmp");
    // 64 KiB of credit lasts ≈ 0.15 s at this rate; the pause is longer.
    let pause = Some((Duration::from_millis(300), Duration::from_millis(400)));
    let (l, c) = pair(
        tmp.path(),
        spec(Role::Send, 1.2, None),
        spec(Role::Receive, 1.2, pause),
    );
    let (tx, rx) = (l.expect("sender"), c.expect("receiver"));
    let sent = tx.sent.expect("sender's counts");
    let got = rx.received.expect("receiver's counts");
    assert_eq!(sent.blocks, expected_blocks(1.2), "no block skipped");
    assert!(sent.stalls >= 1, "{sent:?}");
    assert!(sent.stall_ms > 100.0, "{sent:?}");
    assert!(sent.credit_waits > 0 && sent.late_sends > 0, "{sent:?}");
    assert_eq!(sent.queue_refusals, 0);
    assert_eq!(got.received, sent.blocks);
    assert_eq!(got.missing, 0);
    assert!(got.in_band.stalls >= 1, "{:?}", got.in_band);
    let sum = got.summary.expect("summary");
    // Blocks held back by the stall arrive late against their schedule.
    assert!(sum.vs_schedule.max_us > 100_000.0, "{sum:?}");
}

#[test]
fn two_senders_are_refused_at_the_handshake() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (l, c) = pair(
        tmp.path(),
        spec(Role::Send, 0.2, None),
        spec(Role::Send, 0.2, None),
    );
    assert!(matches!(l, Err(NetError::Handshake(_))), "{l:?}");
    assert!(matches!(c, Err(NetError::Handshake(_))), "{c:?}");
}

#[test]
fn a_directory_holding_a_run_is_refused() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("meta.json"), "{}").expect("write");
    let ep = listen("127.0.0.1:0").expect("listen");
    let r = run(
        ep,
        &spec(Role::Receive, 0.1, None),
        tmp.path(),
        provenance("l"),
        &AtomicBool::new(false),
    );
    assert!(matches!(r, Err(NetError::Exists(_))), "{r:?}");
}

#[test]
fn the_schedule_is_exact_over_long_runs() {
    // 375 blocks of 256 frames at 96 kHz are exactly one second, however far into a run.
    assert_eq!(offset_ns(375, 256, 96_000), 1_000_000_000);
    assert_eq!(offset_ns(375 * 3600 * 24, 256, 96_000), 86_400_000_000_000);
    assert_eq!(offset_ns(1, 256, 48_000), 5_333_333);
}

#[test]
fn quantiles_are_nearest_rank() {
    let mut v: Vec<i64> = (1..=1000).map(|i| i * 1000).collect();
    let q = quantiles(&mut v);
    assert_eq!(
        (q.p50_us, q.p99_us, q.p999_us, q.max_us),
        (500.0, 990.0, 999.0, 1000.0)
    );
    assert_eq!(quantiles(&mut []), Quantiles::default());
}

#[test]
fn a_gap_is_counted_and_listed() {
    let s = spec(Role::Receive, 1.0, None);
    let mut rx = Receiver::new(&s, 10);
    for seq in [0u64, 1, 4, 5, 3] {
        rx.push(&stats::Arrival {
            seq,
            scheduled_ns: seq * 1000,
            sent_ns: seq * 1000,
            arrived_ns: seq * 1000 + 50,
            stalls: 0,
            late_sends: 0,
        });
    }
    let e = rx.finish(Ended::Deadline, 1.0, None);
    assert_eq!((e.received, e.missing, e.reordered), (5, 1, 1));
    assert_eq!(e.gaps, vec![[2, 3]]);
}
