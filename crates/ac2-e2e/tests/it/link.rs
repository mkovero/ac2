//! The app's daemon link (no window) against real daemons on the simulated rig: quitting
//! stops and releases the stimulus when the daemon answers, and never waits longer than
//! `QUIT_GRACE` when it does not (the daemon's lease expiry is the safety net); preview
//! requests keep their order; the Leq log exports a page at a time; a silent daemon is
//! dropped and connected anew.
//!
//! A daemon that stops answering without closing its connections (a half-open link after a
//! lost network, a host frozen) is a TCP relay the test freezes: the link's connections stay
//! open, nothing moves either way.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ac2_client::{ClientConfig, Endpoints};
use ac2_e2e::{Setup, SimDaemon, sim_daemon};
use ac2_proto::model::{GenAction, Generator, GeneratorSettings, Signal};
use ac2_proto::units::Dbfs;
use ac2_ui::conn::{Conn, ConnEvent, QUIT_GRACE, Request, StimEvent, Target};

use crate::harness::{Cli, R};

/// A relay of a daemon's two endpoints that can be frozen: while frozen it holds every
/// connection open and passes nothing either way.
struct Relay {
    endpoints: Endpoints,
    frozen: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    /// Connections accepted on the ctrl endpoint: one per connection the link makes.
    ctrl_accepted: Arc<AtomicUsize>,
}

impl Relay {
    fn to(daemon: &Endpoints) -> R<Self> {
        let frozen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let ctrl_accepted = Arc::new(AtomicUsize::new(0));
        let ctrl = relay(&daemon.ctrl, &frozen, &stop, Some(&ctrl_accepted))?;
        let data = relay(&daemon.data, &frozen, &stop, None)?;
        Ok(Self {
            endpoints: Endpoints { ctrl, data },
            frozen,
            stop,
            ctrl_accepted,
        })
    }

    fn freeze(&self, on: bool) {
        self.frozen.store(on, Ordering::Release);
    }

    fn accepted(&self) -> usize {
        self.ctrl_accepted.load(Ordering::Acquire)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Listens on a loopback port of its own and relays every connection to `target`
/// (`tcp://host:port`); its endpoint.
fn relay(
    target: &str,
    frozen: &Arc<AtomicBool>,
    stop: &Arc<AtomicBool>,
    accepted: Option<&Arc<AtomicUsize>>,
) -> R<String> {
    let to = target
        .strip_prefix("tcp://")
        .ok_or_else(|| format!("not TCP: {target}"))?
        .to_owned();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let endpoint = format!("tcp://{}", listener.local_addr()?);
    let (frozen, stop, accepted) = (frozen.clone(), stop.clone(), accepted.cloned());
    std::thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((a, _)) => {
                    if let Some(n) = &accepted {
                        n.fetch_add(1, Ordering::AcqRel);
                    }
                    let Ok(b) = TcpStream::connect(&to) else {
                        continue;
                    };
                    for (from, into) in [(&a, &b), (&b, &a)] {
                        let (Ok(from), Ok(into)) = (from.try_clone(), into.try_clone()) else {
                            continue;
                        };
                        let (frozen, stop) = (frozen.clone(), stop.clone());
                        std::thread::spawn(move || pump(from, into, &frozen, &stop));
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
    Ok(endpoint)
}

/// Copies `from` into `into` until either closes or the relay stops; frozen, reads nothing.
fn pump(mut from: TcpStream, mut into: TcpStream, frozen: &AtomicBool, stop: &AtomicBool) {
    let _ = from.set_nonblocking(false);
    let _ = from.set_read_timeout(Some(Duration::from_millis(10)));
    let mut buf = [0u8; 16384];
    while !stop.load(Ordering::Acquire) {
        if frozen.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                // Bytes read just as the relay froze are held, not passed, until it thaws.
                while frozen.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if into.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
    }
    let _ = into.shutdown(Shutdown::Both);
    let _ = from.shutdown(Shutdown::Both);
}

fn link(endpoints: Endpoints) -> R<Conn> {
    link_with(ClientConfig::new(endpoints, "ac2-ui link test"))
}

fn link_with(config: ClientConfig) -> R<Conn> {
    Ok(Conn::start(
        Target {
            config,
            describe: "simulated rig".into(),
        },
        Arc::new(|| {}),
    )?)
}

fn wait_for(conn: &Conn, what: &str, mut ok: impl FnMut(&ConnEvent) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if conn.drain().iter().any(&mut ok) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The generator as `cli` mirrors it, once `ok` holds.
fn generator_until(cli: &Cli, what: &str, ok: impl Fn(&Generator) -> bool) -> Generator {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(g) = cli.state().map(|s| s.generator.clone())
            && ok(&g)
        {
            return g;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Connects and arms (never fires: arming is enough to hold the lease).
fn armed(endpoints: Endpoints) -> R<Conn> {
    let conn = link(endpoints)?;
    wait_for(&conn, "connected", |e| {
        matches!(e, ConnEvent::Connected { .. })
    });
    conn.send(Request::StimArm {
        settings: GeneratorSettings {
            signal: Signal::Pink,
            level: Dbfs(-30.0),
            band: None,
            outputs: vec![0],
        },
        force: false,
    });
    wait_for(&conn, "armed", |e| {
        matches!(e, ConnEvent::Stimulus(StimEvent::Armed))
    });
    Ok(conn)
}

/// The simulated rig as the app sets it up, and a client of the test's own on it.
fn rig() -> R<(SimDaemon, Cli)> {
    let daemon = sim_daemon(Setup::Demo)?;
    let cli = Cli::connect(daemon.endpoints())?;
    Ok((daemon, cli))
}

#[test]
fn quit_stops_and_releases_the_stimulus() -> R {
    let (daemon, cli) = rig()?;
    let conn = armed(daemon.endpoints().clone())?;
    generator_until(&cli, "armed", |g| g.armed);
    assert!(conn.close(), "the link finished within the grace period");
    let g = generator_until(&cli, "released", |g| !g.armed && g.owner.is_none());
    assert_eq!(g.last_action.map(|a| a.action), Some(GenAction::Release));
    Ok(())
}

#[test]
fn quit_waits_at_most_the_grace_period_for_a_silent_daemon() -> R {
    let (daemon, cli) = rig()?;
    let relay = Relay::to(daemon.endpoints())?;
    let conn = armed(relay.endpoints.clone())?;
    generator_until(&cli, "armed", |g| g.armed);
    // The daemon stops answering: stop and release can never complete.
    relay.freeze(true);
    let t0 = Instant::now();
    // The link gives up on the stop at the same deadline, so `close` may see it finish
    // just in time or not; either way the quit takes the grace period and no longer.
    conn.close();
    let took = t0.elapsed();
    assert!(took >= QUIT_GRACE - Duration::from_millis(50), "{took:?}");
    assert!(took <= QUIT_GRACE + Duration::from_millis(500), "{took:?}");
    // The daemon side's lease expiry is what stops the output.
    let g = generator_until(&cli, "the lease expired", |g| !g.armed);
    assert_eq!(g.last_action.map(|a| a.action), Some(GenAction::Expiry));
    Ok(())
}

/// The reducer's preview requests reach the daemon in the order it issued them. Closing and
/// reopening the session dialog sends `preview_stop` then `preview` back to back; were they
/// to cross on the way, the daemon would close the preview just asked for and the meters
/// would stay blank until the next renewal: after the last round the device's meters
/// still arrive.
#[test]
fn preview_requests_reach_the_daemon_in_order() -> R {
    use ac2_proto::model::{BackendKind, DeviceId};
    use ac2_proto::topic::Topic;
    let daemon = sim_daemon(Setup::Empty)?;
    let conn = link(daemon.endpoints().clone())?;
    wait_for(&conn, "connected", |e| {
        matches!(e, ConnEvent::Connected { .. })
    });
    conn.send(Request::Meters(true));
    let dev = DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into());
    let preview = || Request::Preview {
        backend: BackendKind::Fake,
        device: dev.clone(),
        inputs: Vec::new(),
        pairs: Vec::new(),
    };
    let rounds = 100;
    for _ in 0..rounds {
        conn.send(Request::PreviewStop);
        conn.send(preview());
    }
    let mut answered = 0;
    let mut newest = 0;
    let seq = |e: &ConnEvent| match e {
        ConnEvent::Data(d) => d
            .latest
            .get(&Topic::PreviewLevels)
            .map(|f| f.frame.stamp.seq),
        _ => None,
    };
    wait_for(&conn, "every preview answered", |e| {
        if let ConnEvent::Preview { result, .. } = e {
            assert!(result.is_ok(), "{result:?}");
            answered += 1;
        }
        newest = newest.max(seq(e).unwrap_or(0));
        answered == rounds
    });
    // Frames of the preview opened last, after every request was answered.
    let mut later = 0;
    wait_for(&conn, "the device's meters after the last round", |e| {
        if seq(e).is_some_and(|s| s > newest) {
            later += 1;
        }
        later >= 3
    });
    Ok(())
}

/// "Export Leq log…": every row of the meter's log (a loaded session's, longer than a page
/// of `spl.log_get`), read a page at a time off the UI thread, written as the log CSV in a
/// folder made for it; the toast counts the seconds.
#[test]
fn the_leq_log_exports_as_csv_in_a_new_folder() -> R {
    use ac2_proto::model::{
        LeqConfig, MeasConfig, MeasKind, PeakWeighting, SessionRef, SplConfig, SplLogPage,
        SplLogRow, SplLogWhich, TimeWeighting, Weighting,
    };
    use ac2_proto::spl_log::SplLogInfo;
    use ac2_proto::units::{MeasId, Seconds, WallNs};
    use ac2_traces::session::{SavedMeasurement, SavedSplLog, Session};
    let (daemon, cli) = rig()?;
    let meas = MeasId(4);
    let n = SplLogPage::MAX_ROWS as u64 + 5000;
    let t0 = 1_792_000_000_000_000_000;
    let rows: Vec<SplLogRow> = (0..n)
        .map(|k| SplLogRow {
            start: WallNs(t0 + k * 1_000_000_000),
            measured: Seconds(1.0),
            laeq: Dbfs(-20.0),
            lceq: Dbfs(-18.0),
            lzeq: Dbfs(-17.0),
            lcpeak: Dbfs(-5.0),
            lafmax: Dbfs(-12.0),
            sensitivity: None,
            position: None,
        })
        .collect();
    let info = SplLogInfo {
        meas,
        name: "FOH SPL".into(),
        input: 1,
        mic: None,
    };
    let dir = tempfile::tempdir()?;
    let session = dir.path().join("long");
    ac2_traces::session::save(
        &session,
        &Session {
            saved_at: WallNs(t0),
            measurements: vec![SavedMeasurement {
                id: meas,
                config: MeasConfig {
                    scope: ac2_proto::model::MeasScope::Session,
                    name: info.name.clone(),
                    kind: MeasKind::Spl {
                        config: SplConfig {
                            cue: None,
                            bands: None,
                            input: 1,
                            weighting: Weighting::A,
                            time_weighting: TimeWeighting::Fast,
                            peak_weighting: PeakWeighting::C,
                            leq: LeqConfig::default_windows(),
                            position: None,
                        },
                    },
                },
                running: false,
                delay: None,
                colour: None,
                layout: None,
                offset: ac2_proto::units::Db(0.0),
                eq: Vec::new(),
            }],
            spl_logs: vec![SavedSplLog {
                info: info.clone(),
                rows,
                bands: Vec::new(),
            }],
            traces: Vec::new(),
            walks: Vec::new(),
            plans: Vec::new(),
            remote_inputs: Vec::new(),
        },
    )?;
    cli.call(ac2_proto::Command::FileLoad {
        session: SessionRef::Path {
            path: session.to_string_lossy().into_owned(),
        },
        discard: true,
    })?;
    let conn = link(daemon.endpoints().clone())?;
    wait_for(&conn, "connected", |e| {
        matches!(e, ConnEvent::Connected { .. })
    });
    let path = dir.path().join("ac2-exports").join("FOH-SPL-leq.csv");
    conn.send(Request::ExportLeqLog {
        info,
        log: SplLogWhich::Current,
        path: path.clone(),
    });
    let mut said = String::new();
    wait_for(&conn, "the export", |e| match e {
        ConnEvent::Reply { what, .. } if what.contains("written to") => {
            said = what.clone();
            true
        }
        _ => false,
    });
    assert!(
        said.starts_with(&format!("{n} seconds of FOH SPL written to ")),
        "{said}"
    );
    assert!(said.ends_with("FOH-SPL-leq.csv"), "{said}");
    let csv = std::fs::read_to_string(&path)?;
    let data = csv
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .count();
    // The rows and the column header.
    assert_eq!(data, n as usize + 1, "{}", &csv[..300.min(csv.len())]);
    Ok(())
}

#[test]
fn a_silent_daemon_is_dropped_and_connected_anew() -> R {
    // The relay keeps every TCP connection open but passes nothing: the transport looks
    // healthy, as a half-open connection after a lost link does, so only the watchdog
    // notices.
    let daemon = sim_daemon(Setup::Empty)?;
    let relay = Relay::to(daemon.endpoints())?;
    let mut config = ClientConfig::new(relay.endpoints.clone(), "ac2-ui link test");
    config.dead_after = Duration::from_millis(600);
    let conn = link_with(config)?;
    wait_for(&conn, "connected", |e| {
        matches!(e, ConnEvent::Connected { .. })
    });
    let connections = relay.accepted();
    relay.freeze(true);
    // The note and the new connection can arrive in one drain: watch for both in order.
    let mut noted = false;
    wait_for(
        &conn,
        "the watchdog's note, then a new connection",
        |e| match e {
            ConnEvent::Reply {
                what,
                result: Err(m),
            } if what == "daemon" && m.contains("no keepalive for") => {
                // A fresh connection must be heard from again before it counts.
                relay.freeze(false);
                noted = true;
                false
            }
            ConnEvent::Connected { .. } => noted,
            _ => false,
        },
    );
    assert!(
        relay.accepted() > connections,
        "the reconnect must be a new connection with its own hello"
    );
    conn.close();
    Ok(())
}

/// Quitting with two rigs: each link stops and releases its own stimulus, in parallel, so
/// a silent rig costs the quit one grace period, not one per rig, and never keeps the
/// answering rig from being released.
#[test]
fn quit_with_two_rigs_stops_both_within_one_grace_period() -> R {
    let (answering, cli) = rig()?;
    let (silent, _) = rig()?;
    let relay = Relay::to(silent.endpoints())?;
    let links = vec![
        armed(relay.endpoints.clone())?,
        armed(answering.endpoints().clone())?,
    ];
    generator_until(&cli, "armed", |g| g.armed);
    relay.freeze(true);
    let t0 = Instant::now();
    ac2_ui::conn::close_all(links);
    let took = t0.elapsed();
    assert!(took <= QUIT_GRACE + Duration::from_millis(500), "{took:?}");
    let g = generator_until(&cli, "released", |g| !g.armed && g.owner.is_none());
    assert_eq!(g.last_action.map(|a| a.action), Some(GenAction::Release));
    Ok(())
}
