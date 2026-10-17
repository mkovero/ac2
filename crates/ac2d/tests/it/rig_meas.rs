//! Rig SPL meters on the fake rig: a meter of the rig (`scope: rig`) is kept in the rig
//! settings, not in session files; a load, a new session and a restart keep it with its
//! id; a loaded session's measurements are numbered around it; `rig.monitor_start` is
//! rev-guarded, and with it on a restarted daemon reopens the recorded device capture-only
//! and the meter meters and logs on with no client.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::Path;
use std::time::{Duration, Instant};

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::{AutosaveConfig, Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(10);

fn start(name: &str, dir: &Path) -> (Handle, ac2_audio::FakeBackend) {
    init_log();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc(&unique(name)));
    cfg.lease_expiry = Duration::from_secs(60);
    cfg.session_dir = dir.join("sessions");
    cfg.rig_settings = Some(dir.join("rig.json"));
    cfg.rig_logs = Some(dir.join("rig-logs"));
    cfg.autosave = Some(AutosaveConfig {
        dir: dir.join("autosave"),
        restore: true,
    });
    (Daemon::start(cfg).unwrap(), backend)
}

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

fn rig_meter(name: &str) -> MeasConfig {
    MeasConfig {
        scope: MeasScope::Rig,
        ..spl(name, 1)
    }
}

fn named(name: &str) -> SessionRef {
    SessionRef::Name { name: name.into() }
}

fn save(c: &mut Client, name: &str) {
    c.ok(Command::FileSave {
        session: named(name),
        overwrite: true,
    });
}

fn load(c: &mut Client, name: &str) {
    c.ok(Command::FileLoad {
        session: named(name),
        discard: true,
    });
}

/// The measurements as (id, name, scope), by id.
fn listed(c: &mut Client) -> Vec<(u32, String, MeasScope)> {
    let mut v: Vec<_> = state(c)
        .measurements
        .into_iter()
        .map(|m| (m.id.0, m.config.name, m.config.scope))
        .collect();
    v.sort_by_key(|(id, _, _)| *id);
    v
}

fn rig_json(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join("rig.json")).unwrap()).unwrap()
}

fn log_total(c: &mut Client, meas: MeasId) -> u64 {
    match c.ok(Command::SplLogGet {
        meas,
        log: SplLogWhich::Current,
        from: 0,
        max: 1000,
    }) {
        ReplyBody::SplLogPage(p) => p.total,
        other => panic!("{other:?}"),
    }
}

#[test]
fn rig_meter_survives_loads_and_stays_out_of_session_files() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _backend) = start("rig-meas-load", dir.path());
    let (mut c, _sub) = connect(&h, &[]);

    // A session with a meter numbered 1, saved.
    c.ok(Command::MeasCreate {
        config: spl("FOH", 1),
    });
    save(&mut c, "a");

    // The meter becomes the rig's: it keeps its id, and the rig settings hold it.
    c.update_ok(MeasId(1), rig_meter("FOH"));
    let rj = rig_json(dir.path());
    assert_eq!(rj["meters"].as_array().unwrap().len(), 1);
    assert_eq!(rj["meters"][0]["id"], 1);

    // A session meter beside it; a save holds that one only.
    c.ok(Command::MeasCreate {
        config: transfer("main"),
    });
    save(&mut c, "b");
    let b = ac2_traces::session::load(&dir.path().join("sessions/b")).unwrap();
    let saved: Vec<_> = b
        .measurements
        .iter()
        .map(|m| m.config.name.clone())
        .collect();
    assert_eq!(saved, vec!["main".to_string()]);

    // Loading "a" brings its meter 1 back; the rig meter holds 1, so it is numbered past
    // every id there is, and the rig meter is untouched.
    load(&mut c, "a");
    assert_eq!(
        listed(&mut c),
        vec![
            (1, "FOH".into(), MeasScope::Rig),
            (3, "FOH".into(), MeasScope::Session),
        ]
    );
    // A new session and another load keep it too.
    c.ok(Command::FileNew { discard: true });
    assert_eq!(listed(&mut c), vec![(1, "FOH".into(), MeasScope::Rig)]);
    load(&mut c, "b");
    assert_eq!(
        listed(&mut c),
        vec![
            (1, "FOH".into(), MeasScope::Rig),
            (2, "main".into(), MeasScope::Session),
        ]
    );
    // A rig meter is not session content: the working state is clean after the load.
    assert!(!state(&mut c).working.dirty);

    // Back to the session: the rig settings let it go.
    c.update_ok(MeasId(1), spl("FOH", 1));
    assert!(
        rig_json(dir.path())["meters"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(c);
    h.shutdown();
}

#[test]
fn rig_meters_come_back_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _backend) = start("rig-meas-restart-1", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    c.ok(Command::MeasCreate {
        config: rig_meter("Stage"),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    c.ok(Command::MeasCreate {
        config: transfer("main"),
    });
    drop(c);
    h.shutdown();

    let (h, _backend) = start("rig-meas-restart-2", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let st = state(&mut c);
    assert_eq!(
        listed(&mut c),
        vec![
            (1, "Stage".into(), MeasScope::Rig),
            (2, "main".into(), MeasScope::Session),
        ]
    );
    let m = st.measurements.iter().find(|m| m.id == MeasId(1)).unwrap();
    assert!(m.running, "a running rig meter comes back running");
    assert!(st.spl_logs.iter().any(|l| l.meas == MeasId(1)));
    // New measurements are numbered past both.
    match c.ok(Command::MeasCreate {
        config: spl("new", 0),
    }) {
        ReplyBody::Measurement(m) => assert_eq!(m.id, MeasId(3)),
        other => panic!("{other:?}"),
    }
    drop(c);
    h.shutdown();
}

#[test]
fn monitor_start_is_rev_guarded() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _backend) = start("rig-meas-monitor-rev", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let rig = state(&mut c).rig;
    let stale = Rev(rig.rev.0 + 100);
    let e = c
        .call(Command::RigMonitorStart {
            base: stale,
            on: true,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    c.ok(Command::RigMonitorStart {
        base: rig.rev,
        on: true,
    });
    let now = state(&mut c).rig;
    assert!(now.monitor_at_start);
    assert!(now.rev > rig.rev);
    assert_eq!(rig_json(dir.path())["monitor_at_start"], true);
    // The old base is stale now.
    let e = c
        .call(Command::RigMonitorStart {
            base: rig.rev,
            on: false,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    drop(c);
    h.shutdown();
}

/// With the monitor at start on, a restarted daemon reopens the recorded device capture
/// only, with no client: the rig meter's log carries on from its file and grows, and the
/// generator has nowhere to play.
#[test]
fn monitor_at_start_meters_with_no_client() {
    let dir = tempfile::tempdir().unwrap();
    let (h, backend) = start("rig-meas-monitor-1", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    c.ok(Command::MeasCreate {
        config: rig_meter("Stage"),
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    let rev = state(&mut c).rig.rev;
    c.ok(Command::RigMonitorStart {
        base: rev,
        on: true,
    });
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    let mut d = driver(&backend);
    run(&mut d, 3.5);
    let deadline = Instant::now() + T;
    let logged = loop {
        let n = log_total(&mut c, MeasId(1));
        if n >= 3 {
            break n;
        }
        assert!(Instant::now() < deadline, "seconds never logged");
        run(&mut d, 0.1);
    };
    // A load does not end the rig meter's log.
    c.ok(Command::FileNew { discard: true });
    assert!(log_total(&mut c, MeasId(1)) >= logged);
    let recorded = rig_json(dir.path())["monitor_session"].clone();
    assert_eq!(recorded["output_channels"], 2, "recorded as opened");
    drop(c);
    drop(d);
    h.shutdown();

    let (h, backend) = start("rig-meas-monitor-2", dir.path());
    // No client has asked for anything: the daemon opened the device on its own.
    let mut d = driver(&backend);
    let (mut c, sub) = connect(&h, &[b"d/1/leq"]);
    let st = state(&mut c);
    let open = st
        .session
        .open
        .expect("the monitor at start opened the audio");
    assert_eq!(open.config.output_channels, 0, "capture only");
    assert_eq!(open.config.loopback, None);
    assert!(!st.generator.armed && !st.generator.firing);
    let back = log_total(&mut c, MeasId(1));
    assert!(back >= logged, "the rig log carried on: {back} < {logged}");
    let deadline = Instant::now() + T;
    loop {
        run(&mut d, 1.2);
        let f = sub.frame(
            Duration::from_millis(200),
            |f| matches!(&f.data, ac2_proto::FrameData::Leq(l) if l.meta.logged > back),
        );
        if f.is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "the rig meter never metered");
    }
    // The generator has no output to play on.
    let tok = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    let e = c
        .call(Command::GenSet {
            lease_token: tok,
            desired: GeneratorDesired {
                settings: GeneratorSettings {
                    signal: Signal::Pink,
                    level: Dbfs(-40.0),
                    band: None,
                    outputs: vec![0],
                },
                armed: true,
                firing: true,
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    drop((c, sub, d));
    h.shutdown();
}

/// Reply warnings: `session.close`, and `session.open` of another input device, while a rig
/// meter exists say so in the reply; the same device again, or no rig meter, says nothing.
#[test]
fn session_close_and_device_change_warn_of_rig_meters() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _backend) = start("rig-meas-warnings", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let warned = |c: &mut Client, cmd: Command| {
        let id = c.next_id();
        let r = c.call_req(ac2_proto::Request::new(id, cmd));
        assert!(r.result.is_ok(), "{:?}", r.result);
        r.warnings
    };
    // No rig meter: nothing to warn about.
    assert!(
        warned(
            &mut c,
            Command::SessionOpen {
                config: session(false)
            }
        )
        .is_empty()
    );
    assert!(warned(&mut c, Command::SessionClose).is_empty());

    c.ok(Command::MeasCreate {
        config: rig_meter("Stage"),
    });
    assert!(
        warned(
            &mut c,
            Command::SessionOpen {
                config: session(false)
            }
        )
        .is_empty()
    );
    // The same device again.
    assert!(
        warned(
            &mut c,
            Command::SessionOpen {
                config: session(false)
            }
        )
        .is_empty()
    );
    let dev = state(&mut c).session.open.unwrap().input_device;
    let other = SessionConfig {
        input_device: DeviceSelector::Id { id: dev },
        ..session(false)
    };
    assert_eq!(
        warned(&mut c, Command::SessionOpen { config: other }),
        vec![ac2_proto::Warning::RigMonitorMoved {
            meas: vec![MeasId(1)]
        }]
    );
    assert_eq!(
        warned(&mut c, Command::SessionClose),
        vec![ac2_proto::Warning::RigMonitorBlind {
            meas: vec![MeasId(1)]
        }]
    );
    // Already closed: the meters were blind before this close.
    assert!(warned(&mut c, Command::SessionClose).is_empty());
    drop(c);
    h.shutdown();
}
