//! ac2-client against the real daemon (in-process, fake backend, loopback TCP): connect +
//! hello, mirror sync, a mutation seen through the mirror, a background-refreshed lease,
//! frames through the drain-based `latest`.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, Endpoints, OnDrop};
use ac2_proto::frame::FrameData;
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::{Dbfs, MeasId};
use ac2_proto::{Command, Stream, Subscription, Topic};
use ac2d::Daemon;
use common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac2_client_drives_the_daemon() {
    init_log();
    let h = Daemon::start(config(realtime_rig(), local_tcp())).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep, "ac2d-test"))
        .await
        .unwrap();
    assert!(c.welcome().server.contains("(build "));
    assert_eq!(c.welcome().daemon_incarnation.0, h.incarnation());
    c.wait_synced(Duration::from_secs(5)).await.unwrap();

    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    c.call(Command::MeasCreate {
        config: spl("meter", 0),
    })
    .await
    .unwrap();
    c.call(Command::MeasStart { meas: MeasId(1) })
        .await
        .unwrap();

    // The lease is refreshed by the client in the background, well past the 1.5 s expiry.
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
    c.subscribe(Subscription::Meas(MeasId(1))).unwrap();
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert!(lease.lost().is_none(), "lease kept alive by refreshes");

    let deadline = Instant::now() + Duration::from_secs(10);
    let topic = Topic::Data {
        meas: MeasId(1),
        stream: Stream::Levels,
    };
    let rms = loop {
        let latest = c.latest().unwrap();
        if let Some(f) = latest.get(&topic)
            && let FrameData::Levels(l) = &f.frame.data
            && !f.stale
        {
            break l.rms[0];
        }
        assert!(Instant::now() < deadline, "no fresh levels frame");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!((rms + 20.0).abs() < 2.0, "loopback level {rms}");

    let view = c.view();
    let s = view.state.as_ref().unwrap();
    assert!(s.generator.firing);
    assert_eq!(s.generator.owner, Some(c.client_id()));
    assert_eq!(s.measurements.len(), 1);

    lease.end().await.unwrap();
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}
