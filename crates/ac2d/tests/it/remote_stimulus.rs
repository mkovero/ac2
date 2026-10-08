//! The stimulus driven by a remote client over CURVE, the way a rig runs it: a network-mode
//! daemon on the fake rig in real time, the async client holding the lease. The loopback
//! input (output 1 → input 1) is the output path; an SPL meter on it shows what leaves.
//!
//! Arming and re-routing never reopen the stream (the session epoch, the running meter and,
//! on JACK, every port connection stay); the output plays whenever the state says firing,
//! and an expired lease is visible in the state before a fresh lease plays again.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, Endpoints, OnDrop};
use ac2_proto::frame::{FrameData, SplMeta};
use ac2_proto::model::{
    GenAction, GeneratorDesired, GeneratorSettings, Signal, SplConfig, State, TimeWeighting,
    Weighting,
};
use ac2_proto::units::{Dbfs, Hz, MeasId, SessionEpoch};
use ac2_proto::{Command, ReplyBody, Stream, Topic};
use ac2_zmq::{AuthorizedKeys, CurveClient, KeyPair};
use ac2d::{Daemon, Listen, NetworkSecurity};
use common::*;

const LEVEL: f64 = -20.0;

fn desired(outputs: Vec<u16>, armed: bool, firing: bool) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Dbfs(LEVEL),
            band: None,
            outputs,
        },
        armed,
        firing,
    }
}

/// The newest fresh SPL reading of measurement 1 satisfying `ok`.
async fn spl_until(c: &Client, what: &str, ok: impl Fn(&SplMeta) -> bool) -> SplMeta {
    let topic = Topic::Data {
        meas: MeasId(1),
        stream: Stream::Spl,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(f) = c.latest().unwrap().get(&topic)
            && !f.stale
            && let FrameData::Spl(s) = &f.frame.data
            && ok(&s.meta)
        {
            return s.meta;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn state_until(c: &Client, what: &str, ok: impl Fn(&State) -> bool) -> std::sync::Arc<State> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(s) = c.view().state.clone()
            && ok(&s)
        {
            return s;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn plays(m: &SplMeta) -> bool {
    (m.level - LEVEL).abs() < 0.5
}

fn silent(m: &SplMeta) -> bool {
    m.level < LEVEL - 40.0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_remote_client_fires_reroutes_and_recovers_from_expiry() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let mut keys = AuthorizedKeys::new();
    keys.insert("laptop", laptop.public).unwrap();
    let authorized = dir.path().join("authorized_clients");
    keys.save(&authorized).unwrap();
    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: NetworkSecurity {
            server_key_file: dir.path().join("server.key"),
            authorized_clients_file: authorized,
        },
    };
    let mut cfg = config(realtime_rig(), listen);
    cfg.lease_expiry = Duration::from_millis(600);
    let h = Daemon::start(cfg).unwrap();
    let mut cc = ClientConfig::new(
        Endpoints {
            ctrl: h.ctrl_endpoint().to_owned(),
            data: h.data_endpoint().to_owned(),
        },
        "remote-stimulus-test",
    );
    cc.curve = Some(CurveClient {
        keys: laptop,
        server_key: h.server_public_key().unwrap(),
    });
    let c = Client::connect(cc).await.unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    assert_eq!(c.client_id().0, "laptop", "identity from CURVE");

    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    let ReplyBody::Measurement(m) = c
        .call(Command::MeasCreate {
            config: ac2_proto::model::MeasConfig {
                name: "output path".into(),
                kind: ac2_proto::model::MeasKind::Spl {
                    config: SplConfig::on_input(0, Weighting::Z, TimeWeighting::Fast),
                },
            },
        })
        .await
        .unwrap()
    else {
        panic!("measurement");
    };
    c.call(Command::MeasStart { meas: m.id }).await.unwrap();
    c.subscribe(ac2_proto::Subscription::Topic(Topic::Data {
        meas: m.id,
        stream: Stream::Spl,
    }))
    .unwrap();
    let epoch = state_until(&c, "session", |s| s.session.open.is_some())
        .await
        .session
        .epoch;
    let before = spl_until(&c, "a running meter", |m| m.duration.0 > 0.5).await;
    assert!(silent(&before), "nothing plays before firing: {before:?}");

    // Arm: routes the generator and keeps the stream (and the meter) running.
    let lease = c
        .acquire_lease(false, OnDrop::StopAndRelease)
        .await
        .unwrap();
    lease.set(desired(vec![0], true, false)).await.unwrap();
    let armed = spl_until(&c, "the meter after arming", |m| {
        m.duration.0 > before.duration.0 + 0.3
    })
    .await;
    assert!(silent(&armed), "arming does not emit: {armed:?}");
    let same_epoch = |s: &State| s.session.epoch == epoch;
    assert!(
        same_epoch(&c.view().state.clone().unwrap()),
        "arming never reopens"
    );

    // Fire: the tone leaves while the state says firing.
    lease.set(desired(vec![0], true, true)).await.unwrap();
    spl_until(&c, "the tone on the output path", plays).await;
    let s = c.view().state.clone().unwrap();
    assert!(s.generator.firing && same_epoch(&s));

    // Hold and fire again at once: the stopped source never mutes the new one.
    lease.set(desired(vec![0], true, false)).await.unwrap();
    lease.set(desired(vec![0], true, true)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let m = spl_until(&c, "the tone after a re-fire", plays).await;
    assert!(plays(&m));
    assert!(c.view().state.clone().unwrap().generator.firing);

    // Re-route while firing: output 1 goes quiet (the stream stays, nothing reopens) and
    // comes back when routed again.
    lease.set(desired(vec![1], true, true)).await.unwrap();
    spl_until(&c, "output 1 silent once routed away", silent).await;
    lease.set(desired(vec![0, 1], true, true)).await.unwrap();
    spl_until(&c, "the tone back on output 1", plays).await;
    let s = c.view().state.clone().unwrap();
    assert!(
        s.generator.firing && same_epoch(&s),
        "re-routing never reopens"
    );
    lease.end().await.unwrap();
    spl_until(&c, "silence after the lease is released", silent).await;

    // A client that stops refreshing: the state shows the expiry and the output is silent.
    let ReplyBody::Lease(l) = c.call(Command::GenAcquire { force: false }).await.unwrap() else {
        panic!("lease");
    };
    c.call(Command::GenSet {
        lease_token: l.lease_token,
        desired: desired(vec![0], true, true),
    })
    .await
    .unwrap();
    spl_until(&c, "the tone under an unrefreshed lease", plays).await;
    let s = state_until(&c, "the expiry", |s| {
        s.generator
            .last_action
            .as_ref()
            .is_some_and(|a| a.action == GenAction::Expiry)
    })
    .await;
    assert!(!s.generator.firing && !s.generator.armed && s.generator.owner.is_none());
    spl_until(&c, "silence after the expiry", silent).await;

    // A fresh lease plays again.
    let lease = c
        .acquire_lease(false, OnDrop::StopAndRelease)
        .await
        .unwrap();
    lease.set(desired(vec![0], true, true)).await.unwrap();
    spl_until(&c, "the tone under a fresh lease", plays).await;
    assert_eq!(
        c.view().state.clone().unwrap().session.epoch,
        SessionEpoch(epoch.0),
        "no reopen at any point"
    );
    lease.end().await.unwrap();
    drop(c);
    h.shutdown();
}
