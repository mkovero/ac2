//! A standalone meter loses power every night: any state file may come back torn,
//! zero-length or garbage. Each kind of damaged file is written here and the daemon started
//! on it: it starts, meters, sets the file aside (never deletes it) and lists it in
//! `rig.damaged_files` for clients.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use ac2_proto::model::*;
use ac2_proto::{Command, ReplyBody};
use ac2d::{AutosaveConfig, Daemon, Handle, Listen, NetworkSecurity};
use common::*;

const T: Duration = Duration::from_secs(10);

/// Ways a power cut leaves a file.
fn damages(good: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("zero-length", Vec::new()),
        ("torn", good[..good.len() / 2].to_vec()),
        ("NUL blocks", vec![0; good.len().max(16)]),
        ("garbage", b"\xff\xfe not this file at all \x01".to_vec()),
    ]
}

fn cfg(name: &str, dir: &Path) -> (ac2d::DaemonConfig, ac2_audio::FakeBackend) {
    init_log();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc(&unique(name)));
    cfg.lease_expiry = Duration::from_secs(60);
    cfg.session_dir = dir.join("sessions");
    cfg.rig_settings = Some(dir.join("rig.json"));
    cfg.rig_logs = Some(dir.join("rig-logs"));
    cfg.cal_store = Some(dir.join("calibrations.json"));
    cfg.autosave = Some(AutosaveConfig {
        dir: dir.join("autosave"),
        restore: true,
    });
    (cfg, backend)
}

fn start(name: &str, dir: &Path) -> (Handle, ac2_audio::FakeBackend) {
    let (cfg, backend) = cfg(name, dir);
    (
        Daemon::start(cfg).expect("a damaged file never stops the start"),
        backend,
    )
}

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

fn band_meter() -> MeasConfig {
    let mut m = spl("bands", 1);
    if let MeasKind::Spl { config } = &mut m.kind {
        config.bands = Some(Box::new(BandLeqPreset::Finland545Lf.apply(None)));
    }
    m
}

fn rig_meter(name: &str) -> MeasConfig {
    MeasConfig {
        scope: MeasScope::Rig,
        ..spl(name, 1)
    }
}

/// A daemon run that leaves every state file: a session SPL meter with a band log and
/// logged seconds, a rig meter (renamed once, so that `rig.json` has a previous copy), the
/// working state.
fn seed(dir: &Path) {
    let (h, backend) = start("power-cut-seed", dir);
    let (mut c, _sub) = connect(&h, &[b"evt"]);
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    c.ok(Command::MeasCreate {
        config: band_meter(),
    });
    c.ok(Command::MeasCreate {
        config: rig_meter("stage"),
    });
    let rig = state(&mut c)
        .measurements
        .into_iter()
        .find(|m| m.config.scope == MeasScope::Rig)
        .unwrap();
    c.ok(Command::MeasUpdate {
        meas: rig.id,
        config: rig_meter("stage left"),
        base: rig.config_rev,
    });
    for m in state(&mut c).measurements {
        c.ok(Command::MeasStart { meas: m.id });
    }
    let mut d = driver(&backend);
    run(&mut d, 2.5);
    drop(c);
    h.shutdown();
}

/// The restored session is disarmed: opened again, every meter started, a second of audio.
fn meter_on(c: &mut Client, backend: &ac2_audio::FakeBackend) {
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    for m in state(c).measurements {
        if !m.running {
            c.ok(Command::MeasStart { meas: m.id });
        }
    }
    let mut d = driver(backend);
    run(&mut d, 1.0);
    assert!(state(c).measurements.iter().all(|m| m.running));
}

fn damaged(c: &mut Client) -> Vec<DamagedFile> {
    state(c).rig.damaged_files
}

fn kinds(v: &[DamagedFile]) -> Vec<StateFile> {
    let mut k: Vec<_> = v.iter().map(|d| d.kind).collect();
    k.sort_by_key(|k| format!("{k:?}"));
    k
}

fn names(c: &mut Client) -> Vec<String> {
    let mut v: Vec<_> = state(c)
        .measurements
        .into_iter()
        .map(|m| m.config.name)
        .collect();
    v.sort();
    v
}

/// Copies the seeded tree to a fresh directory (each damage gets its own).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &p);
        } else {
            std::fs::copy(e.path(), p).unwrap();
        }
    }
}

fn set_aside_exists(p: &Path) -> bool {
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    std::fs::read_dir(p.parent().unwrap())
        .unwrap()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!("{name}.damaged-"))
        })
}

/// The session's SPL log and band log files.
fn log_files(dir: &Path) -> (PathBuf, PathBuf) {
    let a = dir.join("autosave");
    let m = ac2_traces::session::read_manifest(&a).unwrap();
    let l = m
        .spl_logs
        .iter()
        .find(|l| l.bands.is_some())
        .expect("a band meter's log");
    (a.join(&l.file), a.join(l.bands.as_ref().unwrap()))
}

/// A torn, empty or garbage SPL log or band log costs only itself: the meter is restored,
/// meters on into a new log, and the file is moved out of the autosave.
#[test]
fn a_damaged_log_costs_only_itself() {
    let root = tempfile::tempdir().unwrap();
    let seeded = root.path().join("seed");
    seed(&seeded);
    let (spl0, bands0) = log_files(&seeded);
    let (spl_good, bands_good) = (
        std::fs::read(&spl0).unwrap(),
        std::fs::read(&bands0).unwrap(),
    );
    for (i, (what, spl_bad)) in damages(&spl_good).into_iter().enumerate() {
        let dir = root.path().join(format!("run{i}"));
        copy_tree(&seeded, &dir);
        let (spl, bands) = log_files(&dir);
        std::fs::write(&spl, &spl_bad).unwrap();
        let (h, backend) = start("power-cut-log", &dir);
        let (mut c, _sub) = connect(&h, &[b"evt"]);
        assert_eq!(names(&mut c), ["bands", "stage left"], "{what}");
        let d = damaged(&mut c);
        // A torn tail is not damage: the rows before the cut are read.
        if what == "torn" && d.is_empty() {
            drop(c);
            h.shutdown();
            continue;
        }
        assert_eq!(kinds(&d), [StateFile::SplLog], "{what}: {d:?}");
        assert!(d[0].set_aside_as.is_some(), "{what}");
        assert!(
            !spl.exists() || std::fs::read(&spl).unwrap() != spl_bad,
            "{what}"
        );
        meter_on(&mut c, &backend);
        drop(c);
        h.shutdown();
        let _ = bands;
    }
    for (i, (what, bad)) in damages(&bands_good).into_iter().enumerate() {
        let dir = root.path().join(format!("bands{i}"));
        copy_tree(&seeded, &dir);
        let (_, bands) = log_files(&dir);
        std::fs::write(&bands, &bad).unwrap();
        let (h, _backend) = start("power-cut-bands", &dir);
        let (mut c, _sub) = connect(&h, &[b"evt"]);
        assert_eq!(names(&mut c), ["bands", "stage left"], "{what}");
        let d = damaged(&mut c);
        if what == "torn" && d.is_empty() {
            drop(c);
            h.shutdown();
            continue;
        }
        assert_eq!(kinds(&d), [StateFile::BandLog], "{what}: {d:?}");
        drop(c);
        h.shutdown();
    }
}

/// A damaged current manifest: the previous one is restored. Both damaged: an empty
/// session, the autosave set aside. Either way the rig meter, from the rig settings, meters.
#[test]
fn a_damaged_autosave_manifest_never_stops_the_start() {
    let root = tempfile::tempdir().unwrap();
    let seeded = root.path().join("seed");
    seed(&seeded);
    let cur = seeded.join("autosave").join(ac2_traces::session::MANIFEST);
    let good = std::fs::read(&cur).unwrap();
    for (i, (what, bad)) in damages(&good).into_iter().enumerate() {
        let dir = root.path().join(format!("cur{i}"));
        copy_tree(&seeded, &dir);
        let a = dir.join("autosave");
        let prev = a.join(ac2_traces::session::PREV_MANIFEST);
        let cur = a.join(ac2_traces::session::MANIFEST);
        std::fs::write(&cur, &bad).unwrap();
        let has_prev = prev.exists();
        let (h, _backend) = start("power-cut-manifest", &dir);
        let (mut c, _sub) = connect(&h, &[b"evt"]);
        let d = damaged(&mut c);
        assert_eq!(kinds(&d), [StateFile::Autosave], "{what}: {d:?}");
        assert!(names(&mut c).contains(&"stage left".to_owned()), "{what}");
        if has_prev {
            assert!(set_aside_exists(&cur), "{what}");
        }
        drop(c);
        h.shutdown();

        // Both manifests damaged.
        let dir = root.path().join(format!("both{i}"));
        copy_tree(&seeded, &dir);
        let a = dir.join("autosave");
        std::fs::write(a.join(ac2_traces::session::MANIFEST), &bad).unwrap();
        std::fs::write(a.join(ac2_traces::session::PREV_MANIFEST), &bad).unwrap();
        let (h, _backend) = start("power-cut-both", &dir);
        let (mut c, _sub) = connect(&h, &[b"evt"]);
        assert_eq!(names(&mut c), ["stage left"], "{what}");
        assert_eq!(kinds(&damaged(&mut c)), [StateFile::Autosave], "{what}");
        assert!(set_aside_exists(&a), "{what}");
        drop(c);
        h.shutdown();
    }
}

/// `rig.json`, `calibrations.json`, `working.json` and the rig logs' manifest, each
/// damaged: the daemon starts, the rig meter comes back from the previous copy of the rig
/// settings, and each file is listed.
#[test]
fn damaged_small_stores_never_stop_the_start() {
    let root = tempfile::tempdir().unwrap();
    let seeded = root.path().join("seed");
    seed(&seeded);
    let rig_good = std::fs::read(seeded.join("rig.json")).unwrap();
    for (i, (what, bad)) in damages(&rig_good).into_iter().enumerate() {
        let dir = root.path().join(format!("run{i}"));
        copy_tree(&seeded, &dir);
        for f in [
            dir.join("rig.json"),
            dir.join("calibrations.json"),
            dir.join("autosave").join("working.json"),
            dir.join("rig-logs").join(ac2_traces::session::MANIFEST),
        ] {
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, &bad).unwrap();
        }
        let (h, backend) = start("power-cut-stores", &dir);
        let (mut c, _sub) = connect(&h, &[b"evt"]);
        let d = damaged(&mut c);
        let mut want = vec![
            StateFile::Calibrations,
            StateFile::RigLogs,
            StateFile::RigSettings,
            StateFile::Working,
        ];
        want.sort_by_key(|k| format!("{k:?}"));
        assert_eq!(kinds(&d), want, "{what}: {d:?}");
        let rig = d.iter().find(|d| d.kind == StateFile::RigSettings).unwrap();
        assert!(rig.reason.contains("previous copy"), "{}", rig.reason);
        // The rig meter is back from the copy before the last write.
        let n = names(&mut c);
        assert!(n.len() == 2 && n[1].starts_with("stage"), "{what}: {n:?}");
        assert!(set_aside_exists(&dir.join("rig.json")), "{what}");
        assert!(set_aside_exists(&dir.join("calibrations.json")), "{what}");
        // Meters on, and the rig settings are writable again.
        meter_on(&mut c, &backend);
        let st = state(&mut c);
        let m = st
            .measurements
            .iter()
            .find(|m| m.config.scope == MeasScope::Rig)
            .unwrap();
        c.ok(Command::MeasUpdate {
            meas: m.id,
            config: rig_meter("stage right"),
            base: m.config_rev,
        });
        drop(c);
        h.shutdown();
    }
}

/// Network mode: a damaged server key is replaced by a new pair (clients must pin it) and
/// authorized-clients lines that do not parse are dropped; the daemon starts either way.
#[test]
fn damaged_network_keys_never_stop_the_start() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let laptop = ac2_zmq::KeyPair::generate().unwrap();
    let key = dir.path().join("server.key");
    let authorized = dir.path().join("authorized_clients");
    std::fs::write(&key, b"public 0123\0\0\0").unwrap();
    std::fs::write(
        &authorized,
        format!("laptop {}\nphone zzz\ntorn-li", laptop.public.to_z85()),
    )
    .unwrap();
    let listen = Listen::Network {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
        security: NetworkSecurity {
            server_key_file: key.clone(),
            authorized_clients_file: authorized.clone(),
        },
    };
    let h = Daemon::start(config(manual_rig(), listen)).expect("starts");
    let server_key = h.server_public_key().expect("a new key");
    let ctx = ac2_zmq::Context::new().unwrap();
    let ok = ac2_zmq::CurveClient {
        keys: laptop,
        server_key,
    };
    let mut c = Client::connect_with(&ctx, h.ctrl_endpoint(), Some(&ok));
    let d = damaged(&mut c);
    let mut want = vec![StateFile::AuthorizedClients, StateFile::ServerKey];
    want.sort_by_key(|k| format!("{k:?}"));
    assert_eq!(kinds(&d), want, "{d:?}");
    assert!(set_aside_exists(&key) && set_aside_exists(&authorized));
    drop(c);
    h.shutdown();
    let _ = T;
}

/// A damaged trace file costs only that trace: its measurement and the rest come back.
#[test]
fn a_damaged_trace_costs_only_itself() {
    let root = tempfile::tempdir().unwrap();
    let seeded = root.path().join("seed");
    {
        let (h, backend) = start("power-cut-trace-seed", &seeded);
        let (mut c, _sub) = connect(&h, &[b"evt"]);
        super::autosave::capture(&h, &backend, &mut c);
        drop(c);
        h.shutdown();
    }
    let m = ac2_traces::session::read_manifest(&seeded.join("autosave")).unwrap();
    let file = m.traces[0].file.clone();
    let good = std::fs::read(seeded.join("autosave").join(&file)).unwrap();
    for (i, (what, bad)) in damages(&good).into_iter().enumerate() {
        let dir = root.path().join(format!("run{i}"));
        copy_tree(&seeded, &dir);
        std::fs::write(dir.join("autosave").join(&file), &bad).unwrap();
        let (h, _backend) = start("power-cut-trace", &dir);
        let (mut c, _sub) = connect(&h, &[b"evt"]);
        let st = state(&mut c);
        assert_eq!(st.measurements.len(), 1, "{what}");
        assert!(st.traces.is_empty(), "{what}");
        assert_eq!(kinds(&st.rig.damaged_files), [StateFile::Trace], "{what}");
        assert!(dir.join("autosave.damaged-files").is_dir(), "{what}");
        drop(c);
        h.shutdown();
    }
}
