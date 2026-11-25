//! A transfer function across two rigs whose interfaces are not on one clock: two
//! network-mode daemons on simulated devices, the app on the one taking the other's input.

use crate::harness::{Driver, R};
use ac2_client::{Client, ClientConfig, Endpoints};
use ac2_proto::model::{
    MeasConfig, MeasKind, MeasScope, REMOTE_BASE, RemoteInput, RigPeer, SessionConfig, SystemClock,
    TransferConfig,
};
use ac2_proto::units::RigId;
use ac2_proto::{Command, ReplyBody};
use ac2_zmq::{AuthorizedKeys, CurveClient, KeyPair};
use std::time::{Duration, Instant};

/// A rig's interface `ppm` fast against true time: its loop return on in 1, a room path to
/// in 2. Each rig's own interface stays within the bound against its (simulated, synced)
/// system clock; two of them, one fast and one slow, are further apart than either.
pub(crate) fn device(ppm: f64) -> R<std::sync::Arc<dyn ac2_audio::Backend>> {
    use ac2_audio::fake::{FakeConfig, FakeDrive, FakePath, Pace};
    const LOOP: u32 = 32;
    let backend = ac2_audio::FakeBackend::new(FakeConfig {
        drive: FakeDrive::Thread(Pace::Realtime),
        paths: vec![
            FakePath::loopback(0, 0, LOOP),
            FakePath::acoustic(0, 1, LOOP + 240, vec![0.5], 1e-4),
        ],
        audio_clock_ppm: ppm,
        ..FakeConfig::default()
    })?;
    Ok(std::sync::Arc::new(backend))
}

/// A network-mode daemon named `name` on `device`, its system clock stated synced (a
/// relation between rigs needs both clocks disciplined), `app` authorized, `keys` its own.
pub(crate) fn rig(
    dir: &std::path::Path,
    name: &str,
    device: std::sync::Arc<dyn ac2_audio::Backend>,
    app: &KeyPair,
    keys: &KeyPair,
) -> R<ac2d::Handle> {
    let d = dir.join(name);
    std::fs::create_dir_all(&d)?;
    let mut authorized = AuthorizedKeys::new();
    authorized.insert("app", app.public)?;
    authorized.save(&d.join("authorized_clients"))?;
    let key_file = d.join("server.key");
    std::fs::write(
        &key_file,
        format!(
            "public {}\nsecret {}\n",
            keys.public.to_z85(),
            keys.secret.to_z85()
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600))?;
    }
    let listen = ac2d::Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    };
    let mut cfg = ac2d::DaemonConfig::new(std::sync::Arc::clone(&device), listen, -10.0);
    cfg.network = ac2d::NetworkChoice::Fixed(ac2d::NetListen {
        bind: ac2d::NetBind::Addr {
            addr: std::net::Ipv4Addr::LOCALHOST.into(),
        },
        port: ac2d::NetPort::Any,
    });
    cfg.keys = Some(ac2d::NetworkSecurity {
        server_key_file: key_file,
        authorized_clients_file: d.join("authorized_clients"),
    });
    cfg.backends = vec![device];
    cfg.name = name.into();
    cfg.system_clock =
        ac2d::config::SystemClockSource::Stated(SystemClock::Synced { maxerror_us: 100 });
    Ok(ac2d::Daemon::start(cfg)?)
}

/// The network ctrl and data endpoints of a daemon listening there.
/// Once it listens: the network pair is bound beside the local one and may be on its way.
pub(crate) fn net_endpoints(h: &ac2d::Handle) -> (String, String) {
    let t0 = std::time::Instant::now();
    loop {
        match h.network_status() {
            ac2d::NetStatus::Listening { ctrl, data } => return (ctrl, data),
            s if t0.elapsed() > std::time::Duration::from_secs(10) => {
                panic!("the network is not listening: {s:?}")
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}

/// The app's settings for `h`: its endpoints and the CURVE keys.
pub(crate) fn config(h: &ac2d::Handle, app: &KeyPair, name: &str) -> R<ClientConfig> {
    let mut c = ClientConfig::new(
        Endpoints {
            ctrl: net_endpoints(h).0,
            data: net_endpoints(h).1,
        },
        name,
    );
    c.curve = Some(CurveClient {
        keys: app.clone(),
        server_key: h.server_public_key().ok_or("no server key")?,
    });
    Ok(c)
}

pub(crate) fn session(inputs: Vec<u16>, remote: Vec<RemoteInput>) -> SessionConfig {
    SessionConfig {
        backend: None,
        input_device: ac2_proto::model::DeviceSelector::Default,
        output_device: ac2_proto::model::DeviceSelector::Default,
        input_channels: inputs,
        spare_inputs: 0,
        output_channels: 2,
        sample_rate_hz: None,
        buffer_frames: None,
        loopback: None,
        remote_inputs: remote,
        link_offset: None,
    }
}

/// Pupu (its interface 1.6 ppm slow) takes kettu's in 2 (kettu's 1.6 ppm fast) as the
/// measurement input of `Main L`, its own in 1 the reference. Once the rigs have streamed
/// long enough to tell, pupu's daemon finds the interfaces 3.2 ppm apart and stops Main L:
/// kettu's heading in the measurement list says the clocks are not locked, the pane's banner
/// why Main L stopped, its list row *clocks*, the pane's title names the remote side.
#[test]
fn slow_clocks_not_locked_across_rigs() -> R {
    use ac2_ui::state::PaneKind;
    let dir = tempfile::tempdir()?;
    let app = KeyPair::generate()?;
    let pupu_keys = KeyPair::generate()?;
    let kettu_keys = KeyPair::generate()?;
    let kettu = rig(dir.path(), "kettu", device(1.6)?, &app, &kettu_keys)?;
    let pupu = rig(dir.path(), "pupu", device(-1.6)?, &app, &pupu_keys)?;
    let rt = tokio::runtime::Runtime::new()?;
    let (b, a) = rt.block_on(async {
        let b = Client::connect(config(&kettu, &app, "e2e setup")?).await?;
        let a = Client::connect(config(&pupu, &app, "e2e setup")?).await?;
        b.wait_synced(Duration::from_secs(10)).await?;
        a.wait_synced(Duration::from_secs(10)).await?;
        Ok::<_, Box<dyn std::error::Error>>((b, a))
    })?;
    let kettu_id: RigId = b.welcome().rig_id;
    let pupu_id: RigId = a.welcome().rig_id;
    let call = |c: &Client, cmd: Command| -> R<ReplyBody> { Ok(rt.block_on(c.call(cmd))?) };
    call(
        &b,
        Command::SessionOpen {
            config: session(vec![0, 1], Vec::new()),
        },
    )?;
    // Kettu admits pupu as a rig; pupu knows where kettu is and which key it holds.
    call(
        &b,
        Command::AuthAdmitRig {
            key: pupu_keys.public.to_z85(),
            rig_id: pupu_id,
            name: "pupu".into(),
        },
    )?;
    call(
        &a,
        Command::RigPair {
            peer: RigPeer {
                rig: kettu_id,
                name: "kettu".into(),
                ctrl: net_endpoints(&kettu).0,
                data: net_endpoints(&kettu).1,
                key: kettu_keys.public.to_z85(),
            },
        },
    )?;
    call(
        &a,
        Command::SessionOpen {
            config: session(
                vec![0, REMOTE_BASE],
                vec![RemoteInput {
                    slot: 0,
                    rig: kettu_id,
                    rig_name: "kettu".into(),
                    channel: 1,
                }],
            ),
        },
    )?;
    let ReplyBody::Measurement(m) = call(
        &a,
        Command::MeasCreate {
            config: MeasConfig {
                scope: MeasScope::Session,
                name: "Main L".into(),
                kind: MeasKind::Transfer {
                    config: TransferConfig::with_inputs(0, REMOTE_BASE),
                },
            },
        },
    )?
    else {
        return Err("no measurement".into());
    };
    call(&a, Command::MeasStart { meas: m.id })?;

    let mut d = Driver::connect(config(&pupu, &app, crate::harness::NAME)?, "pupu")?;
    let mut p = crate::common::grid_ui_prefs();
    p.layout.panes = p.layout.panes.map(crate::common::rig_picks);
    d.st.set_prefs(p);
    // The relation is judged once the remote input has streamed for a minute.
    let end = Instant::now() + Duration::from_secs(150);
    // Said on kettu's heading in the measurement list, in capitals, after the rate and clock
    // tag that lead the heading.
    let pairing = loop {
        d.pump();
        let rows = d.st.list_rows();
        let said = rows
            .iter()
            .filter(|(_, r)| r.depth == 0 && r.name.starts_with("kettu"))
            .flat_map(|(_, r)| &r.details)
            .find(|t| t.contains("CLOCKS NOT LOCKED"));
        if let Some(t) = said {
            break t.clone();
        }
        if Instant::now() > end {
            return Err(format!(
                "never not locked: rows {rows:?}, remote {:?}",
                d.st.daemon().map(|s| &s.remote)
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(pairing.starts_with("48 kHz FREE · "), "{pairing}");
    let verdict = pairing
        .split(" · ")
        .find(|t| t.starts_with("CLOCKS NOT LOCKED"))
        .ok_or("the verdict")?;
    assert!(verdict.starts_with("CLOCKS NOT LOCKED +3."), "{pairing}");
    let ppm = verdict
        .strip_prefix("CLOCKS NOT LOCKED ")
        .and_then(|t| t.split(" ppm").next())
        .ok_or("the heading's figure")?
        .to_owned();
    // Why and what to do is the detail of the pane's banner; the list row's cue says
    // *clocks* with the pane closed.
    let tf = crate::common::pane(&d.st, PaneKind::Transfer);
    let details = |s: &ac2_ui::state::AppState| {
        let now = ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        };
        let size = ac2_scene::primitives::Viewport::new(1100.0, 600.0);
        let theme = ac2_scene::theme::Theme::dark();
        ac2_ui::scenes::transfer(s, tf, &theme, size, now)
            .banners
            .into_iter()
            .filter_map(|b| b.detail)
            .collect::<Vec<_>>()
    };
    d.until("Main L stopped by the clocks", |s| {
        details(s).iter().any(|b| {
            b.starts_with(&format!(
                "Main L stopped: pupu and kettu are not on one clock ({ppm} ppm)"
            ))
        })
    })?;
    let row = d.st.meas_rows().into_iter().find(|r| r.id == m.id);
    let cue = row.ok_or("Main L's row")?.text;
    assert!(cue.contains("clocks"), "{cue}");
    assert_eq!(d.st.pane_meas(tf).map(|m| m.id), Some(m.id));
    let heading = d.st.pane_heading(tf);
    assert!(heading.ends_with("Main L · meas from kettu"), "{heading}");
    Ok(())
}
