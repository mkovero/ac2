//! The system max level at run time (`gen.ceiling`) on the fake rig: lowering applies at
//! once and stops a stimulus above it (silence observed on the loopback), raising needs a
//! confirmation and a silent generator, the `--max-level` bound is never exceeded, every
//! client sees the change, the level and the output labels survive a restart, and a rig
//! with no level stored starts quiet where loudspeakers may listen.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_audio::FakeDriver;
use ac2_proto::frame::FrameData;
use ac2_proto::model::{
    GenAction, Generator, GeneratorDesired, GeneratorSettings, OutputSetup, Signal,
};
use ac2_proto::units::{Dbfs, MeasId};
use ac2_proto::{Change, Command, ErrorCode};
use ac2d::Daemon;
use common::*;

const T: Duration = Duration::from_secs(5);

fn hello(c: &mut Client) -> String {
    c.welcome(Command::Hello {
        client: "test".into(),
    })
    .client_id
    .0
}

fn fire(level: f64) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::White,
            level: Dbfs(level),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

fn ceiling(c: &mut Client, dbfs: f64, confirm_raise: bool) -> Result<Generator, ErrorCode> {
    c.call(Command::GenCeiling {
        ceiling: Dbfs(dbfs),
        confirm_raise,
    })
    .map(|r| r.generator())
    .map_err(|e| e.code)
}

/// Loopback input meter: a levels subscription that remembers how far it has read.
struct Meter {
    sub: Sub,
    seen_end: u64,
}

impl Meter {
    fn next(&mut self) -> f32 {
        let f = self
            .sub
            .frame(T, |f| matches!(f.data, FrameData::Levels(_)))
            .expect("levels frame");
        let FrameData::Levels(l) = &f.data else {
            unreachable!()
        };
        self.seen_end = f.stamp.audio_sample.0 + 1;
        l.peak[0]
    }

    /// Runs `seconds` of audio; the loopback input's sample peak (dBFS) over exactly that
    /// span.
    fn peak(&mut self, d: &mut FakeDriver, seconds: f64) -> f32 {
        let before = end_sample(d);
        while self.seen_end < before {
            self.next();
        }
        run(d, seconds);
        let end = end_sample(d);
        let mut peak = f32::NEG_INFINITY;
        while self.seen_end < end {
            peak = peak.max(self.next());
        }
        peak
    }
}

#[test]
fn lowering_applies_at_once_raising_needs_a_confirmation_and_silence() {
    init_log();
    let backend = manual_rig();
    let h = Daemon::start(config(backend.clone(), inproc("ceiling"))).unwrap();
    let (mut a, events) = connect(&h, &[b"evt"]);
    let mut m = Meter {
        sub: Sub::connect(h.context(), h.data_endpoint(), &[b"d/1/levels"]),
        seen_end: 0,
    };
    let mut b = Client::connect(h.context(), h.ctrl_endpoint());
    hello(&mut a);
    let id_b = hello(&mut b);
    let g = a.state().generator;
    assert_eq!((g.ceiling, g.ceiling_bound), (Dbfs(-10.0), Dbfs(-10.0)));

    a.ok(Command::StreamOpen {
        config: stream(false),
    });
    a.ok(Command::MeasCreate {
        config: spl("loopback", 0),
    });
    a.ok(Command::MeasStart { meas: MeasId(1) });
    let tok = a.acquire();
    a.ok(Command::GenSet {
        lease_token: tok,
        desired: fire(-20.0),
    });
    let mut d = driver(&backend);
    let peak = m.peak(&mut d, 0.3);
    assert!(peak > -20.0, "playing: {peak}");

    // Above the bound: never.
    assert_eq!(ceiling(&mut b, -6.0, true), Err(ErrorCode::Invalid));

    // B (no lease) lowers it to −15: the stimulus at −20 carries on.
    let g = ceiling(&mut b, -15.0, false).unwrap();
    assert_eq!(g.ceiling, Dbfs(-15.0));
    assert!(g.firing, "below the new maximum: untouched");
    let peak = m.peak(&mut d, 0.2);
    assert!(peak > -20.0, "still playing: {peak}");

    // Lowered below the stimulus: stopped and disarmed at once, the output silent.
    let g = ceiling(&mut b, -30.0, false).unwrap();
    assert_eq!(g.ceiling, Dbfs(-30.0));
    assert!(!g.armed && !g.firing);
    let audit = g.last_action.unwrap();
    assert_eq!(
        (audit.action, audit.client.map(|c| c.0)),
        (GenAction::CeilingLowered, Some(id_b.clone()))
    );
    // The stop came first, in its own event naming the same client.
    events
        .event(T, |e| {
            matches!(&e.change, Change::Generator(g)
                if g.last_action.as_ref().is_some_and(|x| x.action == GenAction::Stop))
        })
        .expect("stop event");
    // A's mirror sees the new maximum (every client does).
    let ev = events
        .event(
            T,
            |e| matches!(&e.change, Change::Generator(g) if g.ceiling == Dbfs(-30.0)),
        )
        .expect("ceiling event");
    let Change::Generator(g) = ev.change else {
        unreachable!()
    };
    assert_eq!(g.ceiling_bound, Dbfs(-10.0));
    let _fade = m.peak(&mut d, 0.05);
    let peak = m.peak(&mut d, 0.2);
    assert_eq!(peak, f32::NEG_INFINITY, "silent after the lowering");
    // Requests above it are refused from now on.
    let e = a
        .call(Command::GenSet {
            lease_token: tok,
            desired: fire(-20.0),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);

    // Raising: refused while armed, refused without the confirmation.
    let mut armed = fire(-35.0);
    armed.firing = false;
    a.ok(Command::GenSet {
        lease_token: tok,
        desired: armed,
    });
    assert_eq!(ceiling(&mut b, -20.0, true), Err(ErrorCode::Refused));
    a.ok(Command::GenStop);
    assert_eq!(ceiling(&mut b, -20.0, false), Err(ErrorCode::Refused));
    let g = ceiling(&mut b, -20.0, true).unwrap();
    assert_eq!(g.ceiling, Dbfs(-20.0));
    let audit = g.last_action.unwrap();
    assert_eq!(
        (audit.action, audit.client.map(|c| c.0)),
        (GenAction::CeilingRaised, Some(id_b))
    );
    a.ok(Command::GenSet {
        lease_token: tok,
        desired: fire(-25.0),
    });
    let _fade = m.peak(&mut d, 0.1);
    let peak = m.peak(&mut d, 0.2);
    assert!(peak > -25.0, "plays again under the raised maximum: {peak}");
    a.ok(Command::GenStop);
    drop(h);
}

#[test]
fn level_and_output_labels_survive_a_restart_under_the_bound() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = |bound: f64| {
        let mut cfg = hardware_config(manual_rig(), inproc("ceiling-restart"));
        cfg.max_level_dbfs = bound;
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let label = |channel: u16, l: &str| OutputSetup {
        channel,
        label: Some(l.into()),
    };

    let h = start(-10.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    ceiling(&mut c, -32.0, true).unwrap();
    let o = c.outputs(Command::StreamOutputs {
        outputs: vec![label(0, "Main L"), label(1, "Sub")],
    });
    assert_eq!(o, vec![label(0, "Main L"), label(1, "Sub")]);
    // A label clears with none; a bad one is refused.
    c.ok(Command::StreamOutputs {
        outputs: vec![OutputSetup {
            channel: 1,
            label: None,
        }],
    });
    let e = c
        .call(Command::StreamOutputs {
            outputs: vec![label(2, " padded")],
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    h.shutdown();

    // The same bound: the level and the labels come back.
    let h = start(-10.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    let s = c.state();
    assert_eq!(s.generator.ceiling, Dbfs(-32.0));
    assert_eq!(s.outputs, vec![label(0, "Main L")]);
    h.shutdown();

    // A lower bound at start wins over the kept level.
    let h = start(-40.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    let g = c.state().generator;
    assert_eq!((g.ceiling, g.ceiling_bound), (Dbfs(-40.0), Dbfs(-40.0)));
    // And the bound refuses anything above it.
    assert_eq!(ceiling(&mut c, -32.0, true), Err(ErrorCode::Invalid));
    h.shutdown();

    // A higher bound again: the capped level was stored, so the older, higher one does
    // not come back without a typed raise.
    let h = start(-6.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    let g = c.state().generator;
    assert_eq!((g.ceiling, g.ceiling_bound), (Dbfs(-40.0), Dbfs(-6.0)));
    ceiling(&mut c, -32.0, true).unwrap();
    h.shutdown();
    let h = start(-6.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(c.state().generator.ceiling, Dbfs(-32.0));
    h.shutdown();
}

#[test]
fn a_restart_with_a_higher_bound_never_raises_the_level_by_itself() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = |bound: f64| {
        let mut cfg = hardware_config(manual_rig(), inproc("ceiling-higher-bound"));
        cfg.max_level_dbfs = bound;
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let stored = || -> serde_json::Value {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&file).unwrap()).unwrap()
            ["ceiling_dbfs"]
            .clone()
    };

    // No file: the quiet start (here the bound too) is the level in force, and it is
    // stored right away.
    let h = start(-50.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(c.state().generator.ceiling, Dbfs(-50.0));
    h.shutdown();
    assert_eq!(stored(), serde_json::json!(-50.0));

    let h = start(-20.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    let g = c.state().generator;
    assert_eq!((g.ceiling, g.ceiling_bound), (Dbfs(-50.0), Dbfs(-20.0)));
    h.shutdown();

    // A file with no level: the quiet start again, under a bound lower still, and stored.
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    v["ceiling_dbfs"] = serde_json::Value::Null;
    std::fs::write(&file, serde_json::to_vec(&v).unwrap()).unwrap();
    let h = start(-60.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(c.state().generator.ceiling, Dbfs(-60.0));
    h.shutdown();
    assert_eq!(stored(), serde_json::json!(-60.0));
}

/// A torn rig settings file is set aside and its previous copy read: the start writes the
/// file again with the level in force and the labels, so the next start finds them there,
/// never a missing file that would start quiet.
#[test]
fn a_recovered_file_is_written_back_at_start() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = |bound: f64| {
        let mut cfg = hardware_config(manual_rig(), inproc("ceiling-recovered"));
        cfg.max_level_dbfs = bound;
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let h = start(-10.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    ceiling(&mut c, -36.0, true).unwrap();
    c.outputs(Command::StreamOutputs {
        outputs: vec![OutputSetup {
            channel: 0,
            label: Some("Main L".into()),
        }],
    });
    h.shutdown();
    // The previous copy holds what is in force; the file itself is torn.
    std::fs::copy(&file, dir.path().join("rig.json.prev")).unwrap();
    std::fs::write(&file, b"{\"ceil").unwrap();

    let h = start(-10.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(c.state().generator.ceiling, Dbfs(-36.0));
    h.shutdown();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["ceiling_dbfs"], serde_json::json!(-36.0));

    // The next start, with a higher bound, reads it back.
    let h = start(-6.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    let s = c.state();
    assert_eq!(s.generator.ceiling, Dbfs(-36.0));
    assert_eq!(s.outputs.len(), 1);
    h.shutdown();
}

/// A start whose level cannot be stored (the settings directory read-only) runs at the
/// level in force; setting that same level then reports the failure instead of claiming it
/// stored, and it does once the file can be written.
#[cfg(unix)]
#[test]
fn a_level_not_stored_at_start_is_stored_when_set() {
    use std::os::unix::fs::PermissionsExt;
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = |bound: f64| {
        let mut cfg = hardware_config(manual_rig(), inproc("ceiling-readonly"));
        cfg.max_level_dbfs = bound;
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let h = start(-20.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    ceiling(&mut c, -20.0, true).unwrap();
    h.shutdown();
    let mode =
        |m: u32| std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o555);
    // Root writes anyway: nothing to test then.
    if std::fs::write(dir.path().join("probe"), b"").is_ok() {
        mode(0o755);
        return;
    }
    // A lower bound caps the level; the cap cannot be stored.
    let h = start(-40.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    assert_eq!(c.state().generator.ceiling, Dbfs(-40.0));
    assert!(ceiling(&mut c, -40.0, false).is_err());
    mode(0o755);
    ceiling(&mut c, -40.0, false).unwrap();
    h.shutdown();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["ceiling_dbfs"], serde_json::json!(-40.0));
}

/// Lowering the level leaves no copy on disk with a higher one: torn after a lowering, the
/// file's previous copy gives the lowered level back, never the one before.
#[test]
fn a_lowered_level_survives_a_torn_file() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = |bound: f64| {
        let mut cfg = hardware_config(manual_rig(), inproc("ceiling-torn-lowered"));
        cfg.max_level_dbfs = bound;
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let h = start(-10.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    ceiling(&mut c, -10.0, true).unwrap();
    ceiling(&mut c, -40.0, false).unwrap();
    h.shutdown();
    std::fs::write(&file, b"{\"ceil").unwrap();

    let h = start(-10.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(c.state().generator.ceiling, Dbfs(-40.0));
    h.shutdown();
}

/// A start whose capped level could not be stored: the next setting saved stores the level
/// in force with it, so a later start with a higher bound keeps it.
#[cfg(unix)]
#[test]
fn another_setting_saved_stores_the_level_in_force() {
    use std::os::unix::fs::PermissionsExt;
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = |bound: f64| {
        let mut cfg = hardware_config(manual_rig(), inproc("ceiling-other-setting"));
        cfg.max_level_dbfs = bound;
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let h = start(-20.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    ceiling(&mut c, -20.0, true).unwrap();
    h.shutdown();
    let mode =
        |m: u32| std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o555);
    if std::fs::write(dir.path().join("probe"), b"").is_ok() {
        mode(0o755);
        return;
    }
    let h = start(-40.0);
    mode(0o755);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    c.outputs(Command::StreamOutputs {
        outputs: vec![OutputSetup {
            channel: 0,
            label: Some("Main L".into()),
        }],
    });
    h.shutdown();

    let h = start(-10.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(c.state().generator.ceiling, Dbfs(-40.0));
    h.shutdown();
}

/// Lowering the level writes the previous copy first: when that write fails the file keeps
/// its level and the lowering is refused, so no copy on disk ever holds more than the
/// level in force.
#[test]
fn a_lowering_that_cannot_write_its_copy_changes_nothing() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = || {
        let mut cfg = hardware_config(manual_rig(), inproc("ceiling-prev-first"));
        cfg.max_level_dbfs = -10.0;
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let h = start();
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    ceiling(&mut c, -20.0, true).unwrap();
    let before = std::fs::read(&file).unwrap();
    // A directory where the copy goes: no file can be written there.
    let prev = dir.path().join("rig.json.prev");
    let _ = std::fs::remove_file(&prev);
    std::fs::create_dir(&prev).unwrap();
    assert!(ceiling(&mut c, -30.0, false).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), before, "the file as it was");
    assert_eq!(c.state().generator.ceiling, Dbfs(-20.0));
    h.shutdown();
}

/// A rig whose outputs may reach loudspeakers (here the fake rig presented as JACK) with no
/// level stored (no file, or one set aside as damaged) starts at −50 dBFS, or at its bound
/// when that is lower, and refuses to play above it; a stored level is taken back, under
/// the bound; and a raise from the quiet start to the bound is stored and survives a
/// restart.
#[test]
fn a_rig_with_no_level_stored_starts_quiet() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = |bound: f64| {
        let mut cfg = hardware_config(manual_rig(), inproc("ceiling-quiet-start"));
        cfg.max_level_dbfs = bound;
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    let in_force = |bound: f64| {
        let h = start(bound);
        let (mut c, _) = connect(&h, &[b"evt"]);
        let g = c.state().generator;
        h.shutdown();
        assert_eq!(g.ceiling_bound, Dbfs(bound));
        g.ceiling.0
    };
    let fresh = || {
        for e in std::fs::read_dir(dir.path()).unwrap().flatten() {
            std::fs::remove_file(e.path()).unwrap();
        }
    };
    let stored_at = |bound: f64, level: f64| {
        fresh();
        let h = start(bound);
        let (mut c, _) = connect(&h, &[b"evt"]);
        hello(&mut c);
        ceiling(&mut c, level, true).unwrap();
        h.shutdown();
    };

    // No file: −50 dBFS reported and enforced.
    let h = start(-20.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(c.state().generator.ceiling, Dbfs(-50.0));
    c.ok(Command::StreamOpen {
        config: stream(false),
    });
    let tok = c.acquire();
    let e = c
        .call(Command::GenSet {
            lease_token: tok,
            desired: fire(-40.0),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
    c.ok(Command::GenSet {
        lease_token: tok,
        desired: fire(-50.0),
    });
    drop(c);
    h.shutdown();
    fresh();
    assert_eq!(in_force(-60.0), -60.0);

    // A damaged file with no previous copy: set aside, and the start is quiet.
    fresh();
    std::fs::write(&file, b"\xff\xfe not a rig file").unwrap();
    let h = start(-20.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    let st = c.state();
    h.shutdown();
    assert_eq!(st.generator.ceiling, Dbfs(-50.0));
    assert_eq!(st.rig.damaged_files.len(), 1, "{:?}", st.rig.damaged_files);
    assert!(
        std::fs::read_dir(dir.path()).unwrap().flatten().any(|e| e
            .file_name()
            .to_string_lossy()
            .starts_with("rig.json.damaged-")),
        "the damaged file set aside"
    );

    // A stored level comes back; one above the bound is capped to it.
    stored_at(-10.0, -30.0);
    assert_eq!(in_force(-20.0), -30.0);
    stored_at(-10.0, -10.0);
    assert_eq!(in_force(-20.0), -20.0);

    // A loud level stored, then the file torn with no previous copy to read: quiet, and
    // the next start stays quiet (nothing louder comes back).
    stored_at(-10.0, -10.0);
    std::fs::remove_file(dir.path().join("rig.json.prev")).unwrap();
    std::fs::write(&file, b"{\"ceil").unwrap();
    assert_eq!(in_force(-10.0), -50.0);
    assert_eq!(in_force(-10.0), -50.0);

    // From the quiet start, one confirmed raise to the bound: stored, and kept.
    fresh();
    let h = start(-20.0);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    assert_eq!(c.state().generator.ceiling, Dbfs(-50.0));
    assert_eq!(ceiling(&mut c, -20.0, false), Err(ErrorCode::Refused));
    assert_eq!(ceiling(&mut c, -20.0, true).unwrap().ceiling, Dbfs(-20.0));
    h.shutdown();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["ceiling_dbfs"], serde_json::json!(-20.0));
    assert_eq!(in_force(-20.0), -20.0);
}

/// A simulated rig (fake backend) with no level stored starts at its bound: nothing outside
/// the process hears it, and it starts ready to measure.
#[test]
fn a_simulated_rig_with_no_level_stored_starts_at_its_bound() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(manual_rig(), inproc("ceiling-simulated-start"));
    cfg.max_level_dbfs = -20.0;
    cfg.rig_settings = Some(dir.path().join("rig.json"));
    let h = Daemon::start(cfg).unwrap();
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(c.state().generator.ceiling, Dbfs(-20.0));
    h.shutdown();
}

/// A simulated rig never stores a max level: a real rig on the same settings file starts
/// as it would without the simulated run, quiet or at what a real rig stored, and a raise
/// on the simulated rig is admitted though not stored.
#[test]
fn a_simulated_rig_never_stores_its_level() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rig.json");
    let start = |hardware: bool| {
        let name = inproc("ceiling-simulated-keeps");
        let mut cfg = if hardware {
            hardware_config(manual_rig(), name)
        } else {
            config(manual_rig(), name)
        };
        cfg.rig_settings = Some(file.clone());
        Daemon::start(cfg).unwrap()
    };
    // The level in force on a real rig, and that a stimulus above it is refused.
    let real_rig = || {
        let h = start(true);
        let (mut c, _) = connect(&h, &[b"evt"]);
        let level = c.state().generator.ceiling.0;
        c.ok(Command::StreamOpen {
            config: stream(false),
        });
        let tok = c.acquire();
        let e = c
            .call(Command::GenSet {
                lease_token: tok,
                desired: fire(level + 10.0),
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Refused, "{e:?}");
        drop(c);
        h.shutdown();
        level
    };

    // A simulated start at its bound, then a real one: quiet.
    let h = start(false);
    let (mut c, _) = connect(&h, &[b"evt"]);
    assert_eq!(c.state().generator.ceiling, Dbfs(-10.0));
    h.shutdown();
    assert_eq!(real_rig(), -50.0);

    // The simulated rig reads the quiet start the real one stored; a confirmed raise there
    // is admitted, and still not stored.
    let h = start(false);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    assert_eq!(c.state().generator.ceiling, Dbfs(-50.0));
    assert_eq!(ceiling(&mut c, -10.0, true).unwrap().ceiling, Dbfs(-10.0));
    h.shutdown();
    assert_eq!(real_rig(), -50.0);

    // A real rig's stored level survives a simulated run that raised the level and saved
    // another setting.
    let h = start(true);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    ceiling(&mut c, -30.0, true).unwrap();
    h.shutdown();
    let h = start(false);
    let (mut c, _) = connect(&h, &[b"evt"]);
    hello(&mut c);
    assert_eq!(c.state().generator.ceiling, Dbfs(-30.0));
    assert_eq!(ceiling(&mut c, -10.0, true).unwrap().ceiling, Dbfs(-10.0));
    c.outputs(Command::StreamOutputs {
        outputs: vec![OutputSetup {
            channel: 0,
            label: Some("Main L".into()),
        }],
    });
    h.shutdown();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["ceiling_dbfs"], serde_json::json!(-30.0));
    assert_eq!(real_rig(), -30.0);
}
