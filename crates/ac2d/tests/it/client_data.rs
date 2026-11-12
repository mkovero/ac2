//! ac2-client's data path against the real daemon, its device run by hand so that frames
//! come exactly while the test plays audio: the newest frame held until it goes STALE,
//! consumers that sleep until data is queued, a frame queued at unsubscribe dropped, grids
//! fetched once. Delta-chain breaks, frames of an old epoch or another daemon and a skewed
//! daemon clock are the unit tests of `ac2_client::data`; chains on the wire are `chain`.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use ac2_audio::{FakeBackend, FakeDriver};
use ac2_client::{Client, OnDrop, StimulusLease};
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::{Dbfs, MeasId};
use ac2_proto::{Command, ErrorCode, GridId, Stream, Subscription, Topic};
use ac2d::{Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(30);
const TF1: Topic = Topic::Data {
    meas: MeasId(1),
    stream: Stream::Tf,
};

/// A transfer measurement on the manual fake device, pink noise round the loopback.
struct TfRig {
    h: Handle,
    d: FakeDriver,
    c: Client,
    lease: StimulusLease,
}

impl TfRig {
    async fn start() -> Self {
        init_log();
        let backend: FakeBackend = manual_rig();
        let h = Daemon::start(steady_config(backend.clone(), local_tcp())).unwrap();
        let c = mirror(&h, "ac2-client-test").await;
        c.call(Command::SessionOpen {
            config: session(true),
        })
        .await
        .unwrap();
        c.call(Command::MeasCreate {
            config: transfer("tf"),
        })
        .await
        .unwrap();
        c.call(Command::MeasStart { meas: MeasId(1) })
            .await
            .unwrap();
        let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
        lease
            .set(GeneratorDesired {
                settings: GeneratorSettings {
                    signal: Signal::Pink,
                    level: Dbfs(-20.0),
                    band: None,
                    outputs: vec![0],
                },
                armed: true,
                firing: true,
            })
            .await
            .unwrap();
        let d = tokio::task::block_in_place(|| driver(&backend));
        Self { h, d, c, lease }
    }

    /// Plays `seconds` of audio, each step once the daemon has taken the last.
    fn play(&mut self, seconds: f64) {
        tokio::task::block_in_place(|| run(&mut self.d, seconds));
    }

    /// The newest `tf` frame's `seq`, playing audio until one arrives.
    async fn first_frame(&mut self) -> u64 {
        for _ in 0..20 {
            self.play(0.25);
            if let Some(t) = self.c.latest().unwrap().get(&TF1) {
                return t.frame.stamp.seq;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("no tf frame");
    }

    /// Drains into `view` until the frame covering all audio played is held and nothing
    /// more is queued: the daemon's analysis may trail the device by a few frames.
    async fn settle(&self, view: &mut ac2_client::Latest) {
        let end = end_sample(&self.d);
        until("frame covering the audio", T, || {
            self.c.latest_into(view).unwrap();
            view.get(&TF1)
                .is_some_and(|t| t.frame.stamp.audio_sample.0 + 1 >= end)
        })
        .await;
        for _ in 0..20 {
            if !self.c.wait_data(Duration::from_millis(150)) {
                return;
            }
            self.c.latest_into(view).unwrap();
        }
        panic!("frames kept coming without audio");
    }

    async fn stop(self) {
        let Self { h, d, c, lease } = self;
        lease.end().await.unwrap();
        drop(c);
        tokio::task::spawn_blocking(move || shutdown_driving(h, Some(d)))
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn latest_keeps_the_newest_frame_until_it_goes_stale() {
    let mut r = TfRig::start().await;
    r.c.subscribe(Subscription::Meas(MeasId(1))).unwrap();
    let first = r.first_frame().await;
    // More audio: only the newest of what queued is kept.
    r.play(0.5);
    let end = end_sample(&r.d);
    until("newest frame", T, || {
        r.c.latest()
            .unwrap()
            .get(&TF1)
            .is_some_and(|t| t.frame.stamp.seq > first && t.frame.stamp.audio_sample.0 + 1 >= end)
    })
    .await;
    let l = r.c.latest().unwrap();
    let tf = l.get(&TF1).unwrap();
    assert!(!tf.stale);
    let age = tf.age.unwrap();
    assert!((0.0..1.0).contains(&age), "age {age}");
    assert_eq!(
        (l.malformed_total, l.refused_total, l.discarded_total),
        (0, 0, 0)
    );
    // No audio, so no frame: STALE after a second.
    until("stale", T, || {
        r.c.latest().unwrap().get(&TF1).is_some_and(|t| t.stale)
    })
    .await;
    // Unsubscribing drops the topic.
    r.c.unsubscribe(Subscription::Meas(MeasId(1))).unwrap();
    assert!(r.c.latest().unwrap().get(&TF1).is_none());
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consumers_wait_for_data_and_reuse_their_view() {
    let mut r = TfRig::start().await;
    r.c.subscribe(Subscription::Topic(TF1)).unwrap();
    r.first_frame().await;
    let mut view = ac2_client::Latest::default();
    r.settle(&mut view).await;
    let name = view.frames.keys().next().cloned().unwrap();
    let before = view.get(&TF1).unwrap().frame.stamp.seq;

    // No audio, nothing new: the wait times out, the async wait does not resolve.
    assert!(!r.c.wait_data(Duration::from_millis(100)));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), r.c.data_changed())
            .await
            .is_err()
    );

    // A frame wakes both, and stays pending until drained.
    let waiter = {
        let c = r.c.clone();
        tokio::spawn(async move { c.data_changed().await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    r.play(0.1);
    tokio::time::timeout(T, waiter).await.unwrap().unwrap();
    assert!(r.c.wait_data(Duration::ZERO));
    r.c.latest_into(&mut view).unwrap();
    assert!(view.read >= 1);
    r.settle(&mut view).await;
    assert!(!r.c.wait_data(Duration::ZERO));
    assert!(view.get(&TF1).unwrap().frame.stamp.seq > before);
    // The view is updated in place: the topic's name is the same allocation.
    assert_eq!(view.frames.len(), 1);
    assert!(Arc::ptr_eq(&name, view.frames.keys().next().unwrap()));
    r.stop().await;
}

/// A frame waiting when its topic is dropped is not kept: the socket's readiness check has
/// already taken it in, so the socket's own filter no longer sees it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_waiting_at_unsubscribe_is_not_kept() {
    let mut r = TfRig::start().await;
    r.c.subscribe(Subscription::Topic(TF1)).unwrap();
    r.first_frame().await;
    r.settle(&mut ac2_client::Latest::default()).await;
    r.play(0.1);
    assert!(r.c.wait_data(T), "the frame arrives");
    r.c.unsubscribe(Subscription::Topic(TF1)).unwrap();
    assert!(
        r.c.latest().unwrap().get(&TF1).is_none(),
        "kept after unsubscribe"
    );
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grids_are_fetched_once_for_unknown_ids() {
    let mut r = TfRig::start().await;
    r.c.subscribe(Subscription::Topic(TF1)).unwrap();
    r.first_frame().await;
    let gid = r.c.latest().unwrap().get(&TF1).unwrap().frame.stamp.grid_id;
    let gid = gid.expect("a tf frame names its grid");
    assert!(!r.c.grid_cached(gid));
    let (_, grids) = r.c.latest_with_grids().await.unwrap();
    assert_eq!(grids.get(&gid).map(|g| g.id()), Some(gid));
    assert!(r.c.grid_cached(gid));
    let (_, again) = r.c.latest_with_grids().await.unwrap();
    assert!(Arc::ptr_eq(&grids[&gid], &again[&gid]), "fetched once");
    let missing = r.c.grid(GridId(42)).await.err().and_then(|e| e.code());
    assert_eq!(missing, Some(ErrorCode::NotFound));
    r.stop().await;
}
