//! Client against the in-process fake daemon. Every wait is a poll with a deadline.

use std::future::Future;
use std::time::{Duration, Instant};

use ac2_client::fake::{FakeDaemon, FakeOptions};
use ac2_client::{Client, ClientConfig, ClientError, OnDrop, Retry};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{
    Change, Command, ErrorCode, Frame, GridId, Patch, ReplyBody, Stream, Subscription, Topic,
    samples,
};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

const DEADLINE: Duration = Duration::from_secs(10);

async fn until<F, Fut>(what: &str, mut f: F) -> R
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let end = Instant::now() + DEADLINE;
    while Instant::now() < end {
        if f().await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

fn fake() -> R<FakeDaemon> {
    Ok(FakeDaemon::start(FakeOptions::default())?)
}

async fn connect(f: &FakeDaemon) -> R<Client> {
    Ok(Client::connect(ClientConfig::new(f.endpoints(), "ac2-client-test")).await?)
}

fn meas_config(name: &str) -> MeasConfig {
    MeasConfig {
        name: name.into(),
        kind: MeasKind::Spl {
            config: SplConfig {
                input: 2,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
            },
        },
    }
}

fn spl_meas(id: u32, name: &str) -> Measurement {
    Measurement {
        id: MeasId(id),
        config: meas_config(name),
        config_rev: Rev(1),
        running: false,
        frozen: false,
        delay: None,
        grid_id: None,
    }
}

fn names(c: &Client) -> Vec<String> {
    c.view()
        .state
        .as_ref()
        .map(|s| {
            s.measurements
                .iter()
                .map(|m| m.config.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_snapshot_and_live_events() -> R {
    let f = fake()?;
    f.lock()
        .commit(Change::Measurement(Patch::Set(spl_meas(1, "pre"))));
    let c = connect(&f).await?;
    assert!(c.client_id().0.starts_with("local-"));
    let st = c.wait_synced(DEADLINE).await?;
    assert_eq!(st.measurements.len(), 1);
    f.lock()
        .commit(Change::Measurement(Patch::Set(spl_meas(2, "live"))));
    until("live event", || async { c.view().rev == Rev(2) }).await?;
    assert_eq!(names(&c), ["pre", "live"]);
    assert_eq!(c.view().snapshots, 1);
    assert_eq!(c.view().since_requests, 0);
    assert!(c.view().responding(Instant::now()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn missed_final_patch_is_fetched_after_one_second() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    c.wait_synced(DEADLINE).await?;
    let t0 = Instant::now();
    {
        let mut s = f.lock();
        s.publish_events = false;
        s.commit(Change::Measurement(Patch::Set(spl_meas(1, "silent"))));
    }
    until("missed patch", || async { c.view().rev == Rev(1) }).await?;
    // Not before the 1 s grace: a ka may legitimately run ahead of its event briefly.
    assert!(t0.elapsed() >= Duration::from_secs(1), "{:?}", t0.elapsed());
    assert_eq!(names(&c), ["silent"]);
    assert!(c.view().since_requests >= 1);
    assert_eq!(f.executions("state.since"), c.view().since_requests as u32);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn gap_is_filled_with_state_since() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    c.wait_synced(DEADLINE).await?;
    {
        let mut s = f.lock();
        s.publish_events = false;
        s.commit(Change::Measurement(Patch::Set(spl_meas(1, "lost"))));
        s.publish_events = true;
        s.commit(Change::Measurement(Patch::Set(spl_meas(2, "seen"))));
    }
    until("gap filled", || async { c.view().rev == Rev(2) }).await?;
    assert_eq!(names(&c), ["lost", "seen"]);
    assert_eq!(f.executions("state.since"), 1);
    assert_eq!(c.view().snapshots, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resync_required_takes_a_new_snapshot() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    c.wait_synced(DEADLINE).await?;
    {
        let mut s = f.lock();
        s.publish_events = false;
        s.commit(Change::Measurement(Patch::Set(spl_meas(1, "a"))));
        s.commit(Change::Measurement(Patch::Set(spl_meas(2, "b"))));
        s.evict_replay();
        s.publish_events = true;
        s.commit(Change::Measurement(Patch::Set(spl_meas(3, "c"))));
    }
    until("resync", || async { c.view().rev == Rev(3) }).await?;
    assert_eq!(names(&c), ["a", "b", "c"]);
    assert_eq!(c.view().snapshots, 2);
    assert_eq!(f.executions("state.snapshot"), 2);
    let st = c.view().state.clone().ok_or("no state")?;
    assert_eq!(*st, f.lock().state);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn incarnation_change_resets_and_rehellos() -> R {
    let f = fake()?;
    f.lock()
        .commit(Change::Measurement(Patch::Set(spl_meas(1, "old"))));
    let c = connect(&f).await?;
    c.wait_synced(DEADLINE).await?;
    let first = c.view().incarnation;
    {
        let mut s = f.lock();
        s.restart(0xbeef);
        s.commit(Change::Measurement(Patch::Set(spl_meas(1, "new"))));
    }
    until("new incarnation", || async {
        let v = c.view();
        v.incarnation == Some(DaemonIncarnation(0xbeef)) && v.synced()
    })
    .await?;
    assert_ne!(first, c.view().incarnation);
    assert_eq!(names(&c), ["new"]);
    assert_eq!(c.view().incarnation_changes, 1);
    until("welcome refreshed", || async {
        c.welcome().daemon_incarnation == DaemonIncarnation(0xbeef)
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_reuses_the_id_and_never_executes_twice() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    f.lock().drop_reply_once.insert("meas.create");
    let retry = Retry {
        timeout: Duration::from_millis(300),
        retries: 3,
    };
    let r = c
        .call_with(
            Command::MeasCreate {
                config: meas_config("once"),
            },
            None,
            retry,
        )
        .await?;
    assert!(matches!(r, ReplyBody::Measurement(m) if m.config.name == "once"));
    assert_eq!(f.executions("meas.create"), 1);
    assert_eq!(f.received("meas.create"), 2);
    let ids: Vec<u64> = f
        .lock()
        .requests
        .iter()
        .filter(|(o, _)| *o == "meas.create")
        .map(|(_, id)| *id)
        .collect();
    assert_eq!(ids[0], ids[1]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_after_all_retries() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    f.lock().mute = true;
    let retry = Retry {
        timeout: Duration::from_millis(100),
        retries: 2,
    };
    let e = c
        .call_with(Command::SessionStatus, None, retry)
        .await
        .err()
        .ok_or("expected a timeout")?;
    assert!(
        matches!(
            e,
            ClientError::Timeout {
                op: "session.status",
                attempts: 3
            }
        ),
        "{e}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn expect_rev_conflict() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    f.lock()
        .commit(Change::Measurement(Patch::Set(spl_meas(1, "x"))));
    let e = c
        .call_expect(Command::MeasDelete { meas: MeasId(1) }, Rev(0))
        .await
        .err()
        .ok_or("expected conflict")?;
    assert_eq!(e.code(), Some(ErrorCode::Conflict));
    c.call_expect(Command::MeasDelete { meas: MeasId(1) }, Rev(1))
        .await?;
    Ok(())
}

fn desired(armed: bool, firing: bool) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Pink,
            level: Dbfs(-30.0),
            band: None,
            outputs: vec![0],
        },
        armed,
        firing,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn lease_is_refreshed_while_held_and_stopped_on_drop() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    let lease = c.acquire_lease(false, OnDrop::StopAndRelease).await?;
    let g = lease.set(desired(true, true)).await?;
    assert!(g.firing);
    // Held for well past the 1.5 s expiry.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(f.lock().expiries, 0);
    assert!(f.lock().refreshes >= 3, "refreshes {}", f.lock().refreshes);
    assert!(f.lock().state.generator.firing);
    assert!(lease.lost().is_none());
    drop(lease);
    until("stop + release", || async {
        f.executions("gen.release") == 1 && f.executions("gen.stop") == 1
    })
    .await?;
    let g = f.lock().state.generator.clone();
    assert!(!g.firing && !g.armed && g.owner.is_none());
    assert_eq!(f.lock().expiries, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lease_expires_daemon_side_when_refreshes_stop() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    let lease = c.acquire_lease(false, OnDrop::Release).await?;
    lease.set(desired(true, true)).await?;
    // The network goes away: nothing reaches the daemon any more.
    f.lock().mute = true;
    until("expiry", || async { f.lock().expiries == 1 }).await?;
    let g = f.lock().state.generator.clone();
    assert!(!g.firing && !g.armed && g.owner.is_none());
    f.lock().mute = false;
    // The next refresh is refused: the lease reports itself lost.
    until("lost", || async { lease.lost().is_some() }).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn forced_takeover_marks_lease_lost_and_drop_does_not_stop_new_owner() -> R {
    let f = fake()?;
    let a = connect(&f).await?;
    let b = connect(&f).await?;
    let la = a.acquire_lease(false, OnDrop::StopAndRelease).await?;
    let held = b.acquire_lease(false, OnDrop::Release).await.err();
    assert_eq!(held.and_then(|e| e.code()), Some(ErrorCode::LeaseHeld));
    let lb = b.acquire_lease(true, OnDrop::Release).await?;
    lb.set(desired(true, true)).await?;
    until("a lost", || async { la.lost().is_some() }).await?;
    drop(la);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(f.executions("gen.stop"), 0);
    assert!(f.lock().state.generator.firing);
    lb.end().await?;
    assert!(!f.lock().state.generator.firing);
    Ok(())
}

fn tf_with(f: &FakeDaemon, seq: u64, grid: Option<GridId>) -> Frame {
    let mut fr = samples::tf_frame(false);
    fr.stamp = f.lock().stamp(seq, grid);
    fr
}

const TF1: Topic = Topic::Data {
    meas: MeasId(1),
    stream: Stream::Tf,
};

async fn wait_frame(c: &Client, f: &FakeDaemon, seq: &mut u64) -> R {
    let end = Instant::now() + DEADLINE;
    while Instant::now() < end {
        *seq += 1;
        let fr = tf_with(f, *seq, None);
        f.lock().publish(&fr);
        tokio::time::sleep(Duration::from_millis(20)).await;
        if c.latest()?.get(&TF1).is_some() {
            return Ok(());
        }
    }
    Err("subscription never delivered".into())
}

#[tokio::test(flavor = "multi_thread")]
async fn latest_keeps_newest_and_discards_old_epochs() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    c.wait_synced(DEADLINE).await?;
    c.subscribe(Subscription::Meas(MeasId(1)))?;
    let mut seq = 0;
    wait_frame(&c, &f, &mut seq).await?;
    let base = seq;
    // A backlog in mixed order, an older epoch with a higher seq, another incarnation.
    for s in [base + 3, base + 1, base + 5, base + 2] {
        let fr = tf_with(&f, s, None);
        f.lock().publish(&fr);
    }
    let mut old = tf_with(&f, base + 100, None);
    old.stamp.session_epoch = SessionEpoch(0);
    let mut other = tf_with(&f, base + 200, None);
    other.stamp.daemon_incarnation = DaemonIncarnation(1);
    f.lock().publish(&old);
    f.lock().publish(&other);
    let probe = tf_with(&f, base + 4, None);
    f.lock().publish(&probe);
    until("backlog", || async {
        c.latest().is_ok_and(|l| {
            l.discarded_total >= 2 && l.get(&TF1).map(|t| t.frame.stamp.seq) == Some(base + 5)
        })
    })
    .await?;
    let l = c.latest()?;
    let tf = l.get(&TF1).ok_or("no tf")?;
    assert_eq!(tf.frame.stamp.seq, base + 5);
    assert!(l.discarded_total >= 2);
    assert_eq!(l.malformed_total, 0);
    let age = tf.age.ok_or("no age")?;
    assert!((0.0..1.0).contains(&age), "age {age}");
    assert!(!tf.stale);
    // No new frame for a second: STALE.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let l = c.latest()?;
    assert!(l.get(&TF1).ok_or("tf gone")?.stale);
    // Unsubscribing drops the topic.
    c.unsubscribe(Subscription::Meas(MeasId(1)))?;
    assert!(c.latest()?.get(&TF1).is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn frame_age_uses_the_clock_offset() -> R {
    let f = FakeDaemon::start(FakeOptions {
        clock_skew_ns: 30_000_000_000,
        ..FakeOptions::default()
    })?;
    let c = connect(&f).await?;
    c.wait_synced(DEADLINE).await?;
    let off = c.view().clock_offset_ns.ok_or("no offset")?;
    assert!((off - 30_000_000_000).abs() < 500_000_000, "offset {off}");
    c.subscribe(Subscription::Topic(TF1))?;
    let mut seq = 0;
    wait_frame(&c, &f, &mut seq).await?;
    let l = c.latest()?;
    let age = l.get(&TF1).ok_or("no tf")?.age.ok_or("no age")?;
    // The daemon's clock runs 30 s ahead; without the offset this would be −30 s.
    assert!(age.abs() < 0.5, "age {age}");
    // An old capture is STALE by age even though it just arrived.
    seq += 1;
    let mut fr = tf_with(&f, seq, None);
    fr.stamp.capture_wall_ns = WallNs(fr.stamp.capture_wall_ns.0 - 3_000_000_000);
    f.lock().publish(&fr);
    until("old frame", || async {
        c.latest()
            .ok()
            .and_then(|l| l.get(&TF1).map(|t| t.frame.stamp.seq))
            == Some(seq)
    })
    .await?;
    let l = c.latest()?;
    let t = l.get(&TF1).ok_or("no tf")?;
    assert!(t.stale);
    assert!((t.age.ok_or("no age")? - 3.0).abs() < 0.5);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_not_responding_after_missing_keepalives() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    c.wait_synced(DEADLINE).await?;
    f.lock().ka_paused = true;
    let t0 = Instant::now();
    until("not responding", || async {
        !c.view().responding(Instant::now())
    })
    .await?;
    assert!(t0.elapsed() >= Duration::from_millis(1300));
    assert!(!c.latest()?.responding);
    f.lock().ka_paused = false;
    until("responding", || async {
        c.view().responding(Instant::now())
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn grids_are_fetched_once_for_unknown_ids() -> R {
    let f = fake()?;
    let c = connect(&f).await?;
    c.wait_synced(DEADLINE).await?;
    let grid = samples::log_grid();
    let gid = grid.id();
    f.lock().grids.insert(gid, grid.clone());
    c.subscribe(Subscription::AllData)?;
    let mut seq = 0;
    wait_frame(&c, &f, &mut seq).await?;
    seq += 1;
    let fr = tf_with(&f, seq, Some(gid));
    f.lock().publish(&fr);
    until("grid frame", || async {
        c.latest().ok().is_some_and(|l| l.grid_ids().contains(&gid))
    })
    .await?;
    assert!(!c.grid_cached(gid));
    let (_, grids) = c.latest_with_grids().await?;
    assert_eq!(grids.get(&gid).map(|g| (**g).clone()), Some(grid));
    let (_, again) = c.latest_with_grids().await?;
    assert_eq!(again.len(), 1);
    assert_eq!(f.executions("grid.get"), 1);
    let missing = c.grid(GridId(42)).await.err().and_then(|e| e.code());
    assert_eq!(missing, Some(ErrorCode::NotFound));
    Ok(())
}
