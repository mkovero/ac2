//! A standalone meter loses power every night: any state file may come back torn,
//! zero-length or garbage. Each kind of damaged file is written here and the daemon started
//! on it: it starts, meters, sets the file aside (never deletes it) and lists it in
//! `rig.damaged_files` for clients.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use ac2_proto::model::*;
use ac2_proto::units::MeasId;
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
    meter_for(c, backend, 1.0);
}

fn meter_for(c: &mut Client, backend: &ac2_audio::FakeBackend, seconds: f64) {
    c.ok(Command::SessionOpen {
        config: session(false),
    });
    for m in state(c).measurements {
        if !m.running {
            c.ok(Command::MeasStart { meas: m.id });
        }
    }
    let mut d = driver(backend);
    run(&mut d, seconds);
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

/// Ways a power cut leaves a whole log unreadable (its header gone).
fn header_damages() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("zero-length", Vec::new()),
        ("NUL blocks", vec![0; 4096]),
        ("garbage", b"\xff\xfe not this file at all \x01".to_vec()),
    ]
}

/// An SPL log or band log whose header is gone (empty, zeros, garbage) costs only itself:
/// the meter is restored, meters on into a new log, and the file is moved out of the
/// autosave.
#[test]
fn a_log_without_its_header_costs_only_itself() {
    let root = tempfile::tempdir().unwrap();
    let seeded = root.path().join("seed");
    seed(&seeded);
    for (i, (what, bad)) in header_damages().into_iter().enumerate() {
        for band in [false, true] {
            let dir = root.path().join(format!("run{i}-{band}"));
            copy_tree(&seeded, &dir);
            let (spl, bands) = log_files(&dir);
            let f = if band { &bands } else { &spl };
            std::fs::write(f, &bad).unwrap();
            let (h, backend) = start("power-cut-log", &dir);
            let (mut c, _sub) = connect(&h, &[b"evt"]);
            assert_eq!(names(&mut c), ["bands", "stage left"], "{what}");
            let d = damaged(&mut c);
            let want = if band {
                StateFile::BandLog
            } else {
                StateFile::SplLog
            };
            assert_eq!(kinds(&d), [want], "{what}: {d:?}");
            assert!(d[0].set_aside_as.is_some(), "{what}");
            meter_on(&mut c, &backend);
            drop(c);
            h.shutdown();
        }
    }
}

/// The rows of a session meter's SPL log and band log on disk, and the files' bytes.
fn read_logs(dir: &Path) -> (usize, usize, Vec<u8>, Vec<u8>) {
    let (spl, bands) = log_files(dir);
    let (s, b) = (std::fs::read(spl).unwrap(), std::fs::read(bands).unwrap());
    let sr = ac2_proto::spl_log::import_csv(&s).unwrap().rows.len();
    let br = ac2_traces::band_log::import_csv(&b).unwrap().rows.len();
    (sr, br, s, b)
}

/// Cuts `p` inside its last row (`frac` of the way in) and, with `zeros`, extends it with
/// zero blocks as a file system that committed the length before the data does. Returns
/// the rows left whole.
fn cut_last_row(p: &Path, frac: f64, zeros: bool) -> usize {
    let mut b = std::fs::read(p).unwrap();
    let whole = ac2_traces::band_log::import_csv(&b)
        .map(|r| r.rows.len())
        .or_else(|_| ac2_proto::spl_log::import_csv(&b).map(|r| r.rows.len()))
        .unwrap();
    let last = b[..b.len() - 1].iter().rposition(|&c| c == b'\n').unwrap() + 1;
    let row = b.len() - last;
    let keep = last + 1 + ((row - 2) as f64 * frac) as usize;
    b.truncate(keep);
    if zeros {
        b.extend([0u8; 4096]);
        b.push(b'\n');
        b.extend([0u8; 1024]);
    }
    std::fs::write(p, b).unwrap();
    whole - 1
}

/// The rig meter's SPL log file.
fn rig_log_file(dir: &Path) -> PathBuf {
    let d = dir.join("rig-logs");
    let m = ac2_traces::session::read_manifest(&d).unwrap();
    d.join(&m.spl_logs[0].file)
}

fn no_damaged_files(dir: &Path) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        assert!(!n.contains(".damaged"), "{}", e.path().display());
        if e.file_type().unwrap().is_dir() {
            no_damaged_files(&e.path());
        }
    }
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

/// The nightly case, ten nights over: the log is cut inside its last row (mid-number,
/// mid-field), alone or followed by kilobytes of zeros. Every whole row before the cut is
/// read back into the meter's history, nothing is reported damaged or set aside, the tail
/// is cut off before the next append, and the rows only grow.
#[test]
fn slow_torn_and_zero_filled_tails_keep_every_whole_row() {
    nights(10);
}

/// [`slow_torn_and_zero_filled_tails_keep_every_whole_row`] for one torn and one
/// zero-filled night, in the quick tier.
#[test]
fn a_torn_and_a_zero_filled_tail_keep_every_whole_row() {
    nights(2);
}

fn nights(n: usize) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("rig");
    seed(&dir);
    let mut prev = 0;
    for night in 0..n {
        let (spl, bands) = log_files(&dir);
        let frac = [0.05, 0.5, 0.95, 0.3, 0.7][night % 5];
        let zeros = night % 2 == 1;
        let want = cut_last_row(&spl, frac, zeros);
        let want_bands = cut_last_row(&bands, 1.0 - frac, !zeros);
        let rig_log = rig_log_file(&dir);
        let want_rig = cut_last_row(&rig_log, frac, !zeros);
        assert!(
            want > prev || night == 0,
            "night {night}: {want} after {prev}"
        );
        prev = want;

        let (h, backend) = start("power-cut-night", &dir);
        let (mut c, _sub) = connect(&h, &[b"evt"]);
        let st = state(&mut c);
        assert!(
            st.rig.damaged_files.is_empty(),
            "night {night}: {:?}",
            st.rig.damaged_files
        );
        let meter = st
            .measurements
            .iter()
            .find(|m| m.config.name == "bands")
            .unwrap()
            .id;
        assert_eq!(log_total(&mut c, meter), want as u64, "night {night}");
        // A day of metering: rows appended after the cut.
        meter_for(&mut c, &backend, 3.0);
        drop(c);
        h.shutdown();

        let (rows, band_rows, s, b) = read_logs(&dir);
        assert!(rows > want, "night {night}: {rows} rows after {want}");
        assert!(
            band_rows > want_bands,
            "night {night}: {band_rows} band rows"
        );
        assert!(
            !s.contains(&0) && !b.contains(&0),
            "night {night}: zeros left in a log"
        );
        assert_eq!(s.last(), Some(&b'\n'));
        let r = std::fs::read(rig_log_file(&dir)).unwrap();
        let rig_rows = ac2_proto::spl_log::import_csv(&r).unwrap().rows.len();
        assert!(
            rig_rows > want_rig,
            "night {night}: rig log {rig_rows} after {want_rig}"
        );
        assert!(!r.contains(&0), "night {night}: zeros left in the rig log");
        no_damaged_files(root.path());
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
