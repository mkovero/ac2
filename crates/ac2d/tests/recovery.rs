//! Audio that stops while a session is open (`docs/design/audio-recovery.md`), from an
//! empty daemon on the fake rig: a device that stops delivering without an error is
//! reported within the bound, the daemon keeps answering while its reopen hangs on the
//! device, and once the device is back the same measurements run in a new epoch with the
//! generator disarmed and the SPL log showing the outage as gap; a device that vanishes is
//! reopened with backoff; a close ends the attempts.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_audio::FakeBackend;
use ac2_client::{Client, ClientConfig, Endpoints, OnDrop};
use ac2_proto::frame::{FrameData, LeqRun};
use ac2_proto::model::{
    GeneratorDesired, GeneratorSettings, Recovery, Session, Signal, State, StopCause,
};
use ac2_proto::units::{Dbfs, MeasId};
use ac2_proto::{Command, ReplyBody, Stream, Subscription, Topic};
use ac2d::{Daemon, Handle};
use common::*;

const WAIT: Duration = Duration::from_secs(40);

async fn connect(h: &Handle) -> Client {
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep, "ac2d-recovery-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    c
}

fn state(c: &Client) -> Arc<State> {
    c.view().state.clone().unwrap()
}

async fn until(c: &Client, what: &str, wait: Duration, ok: impl Fn(&State) -> bool) -> Arc<State> {
    let deadline = Instant::now() + wait;
    loop {
        let s = state(c);
        if ok(&s) {
            return s;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            s.session
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// An empty daemon on `backend` with a session open (loopback), a transfer measurement and
/// an SPL meter running.
async fn start(backend: &FakeBackend) -> (Handle, Client, MeasId, MeasId) {
    start_with(backend, None).await
}

async fn start_with(
    backend: &FakeBackend,
    recordings: Option<std::path::PathBuf>,
) -> (Handle, Client, MeasId, MeasId) {
    init_log();
    let mut cfg = config(backend.clone(), local_tcp());
    cfg.recording_dir = recordings;
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let c = connect(&h).await;
    c.call(Command::SessionOpen {
        config: session(true),
    })
    .await
    .unwrap();
    let mut ids = Vec::new();
    for m in [transfer("Main"), spl("FOH SPL", 1)] {
        let id = match c.call(Command::MeasCreate { config: m }).await.unwrap() {
            ReplyBody::Measurement(m) => m.id,
            other => panic!("{other:?}"),
        };
        c.call(Command::MeasStart { meas: id }).await.unwrap();
        ids.push(id);
    }
    (h, c, ids[0], ids[1])
}

/// The SPL meter's run once `ok` holds.
async fn run_until(c: &Client, meas: MeasId, what: &str, ok: impl Fn(&LeqRun) -> bool) -> LeqRun {
    let topic = Topic::Data {
        meas,
        stream: Stream::Leq,
    };
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(f) = c.latest().unwrap().get(&topic)
            && let FrameData::Leq(l) = &f.frame.data
            && let Some(r) = l.meta.run
            && ok(&r)
        {
            return r;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn stopped(s: &Session) -> bool {
    s.stopped.is_some()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_device_is_reported_and_the_session_reopens_by_itself() {
    let backend = realtime_rig();
    let (_h, c, tf, meter) = start(&backend).await;
    c.subscribe(Subscription::Meas(meter)).unwrap();
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    lease
        .set(GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-30.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        })
        .await
        .unwrap();
    run_until(&c, meter, "two seconds logged", |r| r.measured.0 >= 2.0).await;
    let before = state(&c);
    let epoch = before.session.epoch;
    let opened = before.session.open.clone().unwrap();

    // The device stops delivering, without an error: AUDIO STOPPED within the bound.
    let t0 = Instant::now();
    backend.stall(None);
    let s = until(&c, "audio stopped", WAIT, |s| stopped(&s.session)).await;
    let took = t0.elapsed();
    assert!(
        took < Duration::from_millis(1000 + 1500),
        "reported after {took:?}"
    );
    let st = s.session.stopped.clone().unwrap();
    assert_eq!(st.cause, StopCause::NotDelivering { after_ms: 1000 });
    assert_eq!(
        s.session.epoch, epoch,
        "the session stays open in its epoch"
    );
    assert_eq!(s.session.open.as_ref(), Some(&opened));
    // Measurements are paused, not stopped; the generator is disarmed at once.
    let s = until(&c, "generator disarmed", WAIT, |s| !s.generator.armed).await;
    assert!(!s.generator.firing);
    assert!(
        s.measurements.iter().all(|m| m.running),
        "{:?}",
        s.measurements
    );

    // The reopen hangs on the stalled device; the daemon still answers at once.
    let s = until(&c, "attempt 1 opening", WAIT, |s| {
        matches!(
            s.session.stopped.as_ref().map(|x| &x.recovery),
            Some(Recovery::Opening { attempt: 1, .. })
        )
    })
    .await;
    assert!(stopped(&s.session));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let asked = Instant::now();
    match c.call(Command::SessionStatus).await.unwrap() {
        ReplyBody::Session(s) => assert!(s.stopped.is_some()),
        other => panic!("{other:?}"),
    }
    assert!(asked.elapsed() < Duration::from_millis(500));

    // The device is back: the same configuration and measurements, a new epoch, disarmed.
    let back_at = ac2_proto::units::WallNs(wall_now_ns());
    backend.restore();
    let s = until(&c, "reopened", WAIT, |s| {
        s.session.stopped.is_none() && s.session.epoch.0 > epoch.0
    })
    .await;
    let o = s.session.open.clone().unwrap();
    assert_eq!(o.config, opened.config);
    assert!(o.opened_at > opened.opened_at);
    assert!(!s.generator.armed && !s.generator.firing);
    let ids: Vec<(MeasId, bool)> = s.measurements.iter().map(|m| (m.id, m.running)).collect();
    assert_eq!(ids, [(tf, true), (meter, true)]);

    // The meter logs again; the outage is gap time, not spliced.
    let r = run_until(&c, meter, "seconds after the outage", |r| r.until > back_at).await;
    let elapsed = (r.until.0 - r.started_at.0) as f64 / 1e9;
    assert!(r.gaps.0 >= 2.0, "{r:?}");
    assert!((r.measured.0 + r.gaps.0 - elapsed).abs() < 1.5, "{r:?}");
    // The transfer measurement publishes again in the new epoch.
    c.subscribe(Subscription::Meas(tf)).unwrap();
    let topic = Topic::Data {
        meas: tf,
        stream: Stream::Tf,
    };
    let deadline = Instant::now() + WAIT;
    loop {
        if c.latest()
            .unwrap()
            .get(&topic)
            .is_some_and(|f| f.frame.stamp.session_epoch == s.session.epoch)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no transfer frame after the reopen"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_vanished_device_is_reopened_with_backoff_until_it_returns() {
    let backend = realtime_rig();
    let dir = tempfile::tempdir().unwrap();
    let (_h, c, _, _) = start_with(&backend, Some(dir.path().to_owned())).await;
    let epoch = state(&c).session.epoch;
    // A recording across the outage ends with it: the audio after the reopen is never
    // spliced onto the file.
    c.call(Command::RecStart {
        request: ac2_proto::model::RecordRequest {
            inputs: vec![0, 1],
            name: Some("across".into()),
            max_duration: ac2_proto::units::Seconds(60.0),
            max_bytes: None,
        },
    })
    .await
    .unwrap();
    until(&c, "recording", WAIT, |s| {
        s.recording.as_ref().is_some_and(|r| r.frames > 0)
    })
    .await;
    let t0 = Instant::now();
    backend.vanish(Some(Duration::from_millis(3500)));
    let s = until(&c, "host ended", WAIT, |s| stopped(&s.session)).await;
    assert_eq!(
        s.session.stopped.as_ref().unwrap().cause,
        StopCause::HostEnded
    );
    let s = until(&c, "the recording ended", WAIT, |s| {
        s.recording.as_ref().is_some_and(|r| {
            r.status
                == ac2_proto::model::RecordingStatus::Ended {
                    reason: ac2_proto::model::RecordingEnd::AudioStopped,
                }
        })
    })
    .await;
    assert_eq!(s.recording.as_ref().unwrap().name, "across");
    let s = until(&c, "a failed attempt", WAIT, |s| {
        matches!(
            s.session.stopped.as_ref().map(|x| &x.recovery),
            Some(Recovery::Waiting { .. })
        )
    })
    .await;
    match &s.session.stopped.as_ref().unwrap().recovery {
        Recovery::Waiting { error, next_at, .. } => {
            assert!(error.contains("simulated device is gone"), "{error}");
            assert!(next_at.0 > wall_now_ns() - 100_000_000);
        }
        other => panic!("{other:?}"),
    }
    // Attempts at about 0, 1, 3 and 7 s: the device returns at 3.5 s, the fourth opens it.
    let s = until(&c, "reopened", WAIT, |s| s.session.stopped.is_none()).await;
    assert!(s.session.epoch.0 > epoch.0);
    assert!(s.session.open.is_some());
    let took = t0.elapsed();
    assert!(
        took > Duration::from_millis(3500) && took < Duration::from_secs(15),
        "{took:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_ends_the_attempts() {
    let backend = realtime_rig();
    let (_h, c, _, _) = start(&backend).await;
    backend.vanish(None);
    until(&c, "stopped", WAIT, |s| stopped(&s.session)).await;
    c.call(Command::SessionClose).await.unwrap();
    until(&c, "closed", WAIT, |s| {
        s.session.open.is_none() && s.session.stopped.is_none()
    })
    .await;
    backend.restore();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let s = state(&c);
    assert!(
        s.session.open.is_none(),
        "no attempt after the close: {:?}",
        s.session
    );
}
