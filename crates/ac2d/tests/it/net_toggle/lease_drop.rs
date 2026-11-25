//! A network app holding the stimulus loses its connection to the network turned off,
//! moved or failed, or its key revoked: its lease ends with the connection.

use super::*;

use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::{Dbfs, Hz};

fn desired(firing: bool) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Dbfs(-30.0),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing,
    }
}

/// How the network app loses its connection.
#[derive(Clone, Copy, Debug)]
enum Cut {
    /// The network turned off.
    Off,
    /// The network moved to another port.
    Moved,
    /// The app's key revoked.
    Revoked,
    /// The listener failed (its authentication handler died).
    Failed,
}

/// A network app holding the stimulus loses its connection (`cut`): the stimulus it held
/// stops at once, not when its lease would have run out, since the app can no longer
/// refresh or stop it. Turned off, the app is told before its connection ends.
async fn cut_network_app_loses_its_lease(cut: Cut) {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = KeyPair::generate().unwrap();
    let mut cfg = settings_config(realtime_rig(), dir.path(), &laptop, None);
    // Far longer than the wait below: only ending the lease with the network stops it.
    cfg.lease_expiry = Duration::from_secs(600);
    let h = Daemon::start(cfg).unwrap();
    let ctx = Context::new().unwrap();
    let port = turn_on(&mut Client::connect(&ctx, h.ctrl_endpoint()), &h);
    let local = mirror(&h, "local-test").await;
    local
        .call(Command::StreamOpen {
            config: stream(false),
        })
        .await
        .unwrap();

    let mut cc = ac2_client::ClientConfig::new(
        ac2_client::Endpoints {
            ctrl: format!("tcp://127.0.0.1:{port}"),
            data: format!("tcp://127.0.0.1:{}", port + 1),
        },
        "laptop",
    );
    cc.curve = Some(CurveClient {
        keys: laptop,
        server_key: h.server_public_key().unwrap(),
    });
    let remote = ac2_client::Client::connect(cc).await.unwrap();
    remote.wait_synced(Duration::from_secs(5)).await.unwrap();
    let lease = remote
        .acquire_lease(false, ac2_client::OnDrop::StopAndRelease)
        .await
        .unwrap();
    lease.set(desired(true)).await.unwrap();
    let poll = Poll::new(T, 20);
    mirror_until(&local, poll, "the remote app's tone", |s| {
        (s.generator.firing && s.generator.owner.as_ref().is_some_and(|o| o.0 == "laptop"))
            .then_some(())
    })
    .await;
    assert!(!remote.network_turned_off());

    match cut {
        Cut::Off | Cut::Moved => {
            let moved = matches!(cut, Cut::Moved);
            local
                .call(Command::ServerNetwork {
                    enabled: moved,
                    bind: None,
                    port: moved.then(free_pair),
                })
                .await
                .unwrap();
        }
        Cut::Revoked => {
            local
                .call(Command::AuthRevoke {
                    name: "laptop".into(),
                })
                .await
                .unwrap();
        }
        Cut::Failed => h.fail_zap_handler().unwrap(),
    }
    if matches!(cut, Cut::Off) {
        let t0 = Instant::now();
        while !remote.network_turned_off() {
            assert!(t0.elapsed() < T, "the remote app was not told");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    let (s, last) = mirror_until(&local, poll, "the lease ended", |s| {
        (s.generator.owner.is_none() && !s.generator.firing)
            .then(|| (s.server.clone(), s.generator.last_action.clone()))
    })
    .await;
    if matches!(cut, Cut::Off) {
        assert_eq!(s.status, NetStatusKind::Off);
    }
    let last = last.expect("audited");
    assert_eq!(last.action, ac2_proto::model::GenAction::Expiry);
    assert_eq!(last.client.map(|c| c.0), Some("laptop".to_owned()));
    drop(lease);
    drop(remote);
    h.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turning_off_tells_the_network_app_and_ends_its_lease() {
    cut_network_app_loses_its_lease(Cut::Off).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn moving_the_network_ends_a_network_apps_lease() {
    cut_network_app_loses_its_lease(Cut::Moved).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoking_its_key_ends_a_network_apps_lease() {
    cut_network_app_loses_its_lease(Cut::Revoked).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_listener_ends_a_network_apps_lease() {
    cut_network_app_loses_its_lease(Cut::Failed).await;
}
