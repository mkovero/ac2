//! The rig settings file (`rig.json`) across builds: a file of the current format version
//! loads whole; one of another version is set aside and nothing read from it; a meters
//! section that cannot be read costs the meters only, never the ceiling, the outputs, the
//! monitor at start or the Art-Net cues; and what a daemon writes, the next one reads
//! unchanged.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::net::UdpSocket;
use std::path::Path;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2d::{Daemon, Handle};
use common::*;
use serde_json::{Value, json};

fn start(name: &str, dir: &Path) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique(name)));
    cfg.session_dir = dir.join("sessions");
    cfg.rig_settings = Some(dir.join("rig.json"));
    cfg.rig_logs = Some(dir.join("rig-logs"));
    Daemon::start(cfg).unwrap()
}

/// A rig SPL meter with a night limit, a warn margin and a DMX cue.
fn night_meter() -> MeasConfig {
    let dmx = |t: &str| {
        Some(CueAction {
            osc: None,
            dmx: parse_dmx_list(t).unwrap(),
        })
    };
    let MeasKind::Spl { mut config } = spl("FOH night", 1).kind else {
        unreachable!()
    };
    config.weighting = Weighting::A;
    config.leq = LeqConfig {
        windows: vec![LeqWindow {
            cue: Some(Box::new(Cue {
                ok: dmx("1=0"),
                near: dmx("1=128"),
                over: dmx("1=255"),
                fault: dmx("2=255"),
            })),
            duration: Seconds(1.0),
            weighting: Weighting::A,
            limit: Some(DbSpl(96.0)),
            warn_margin: Db(3.0),
        }],
        horizon: Seconds(2.0),
        peaks: Default::default(),
    };
    MeasConfig {
        scope: MeasScope::Rig,
        name: "FOH night".into(),
        kind: MeasKind::Spl { config },
    }
}

struct Seeded {
    meter: MeasConfig,
    target: ArtnetTarget,
    monitor: SessionConfig,
    /// Kept open: the Art-Net frames the daemon sends land here.
    _node: UdpSocket,
}

fn seeded() -> Seeded {
    let node = UdpSocket::bind("127.0.0.1:0").unwrap();
    Seeded {
        meter: night_meter(),
        target: ArtnetTarget {
            addr: node.local_addr().unwrap(),
            universe: PortAddress::new(0).unwrap(),
        },
        monitor: session(false),
        _node: node,
    }
}

/// The format version this build reads.
const VERSION: u32 = 11;

/// A rig settings file of the current version holding the seeded settings and `meters`.
fn file_with_meters(s: &Seeded, meters: Value) -> Value {
    json!({
        "format": "ac2-rig-settings",
        "version": VERSION,
        "ceiling_dbfs": -30.0,
        "outputs": [{ "channel": 0, "label": "Main L" }, { "channel": 3, "label": "Sub" }],
        "meters": meters,
        "monitor_at_start": true,
        "monitor_session": s.monitor,
        "osc_target": null,
        "artnet_target": s.target,
        "artnet_fade": 2.5,
        "levels": LevelsSettings::default(),
        "peers": []
    })
}

/// The seeded night meter as the file keeps it.
fn meter_json(s: &Seeded) -> Value {
    json!({
        "id": 1,
        "config": s.meter,
        "running": true,
        "colour": null,
        "layout": null
    })
}

fn rig_json(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("rig.json")).unwrap()).unwrap()
}

/// Everything but the meters is as seeded.
fn assert_rig_kept(st: &State, s: &Seeded) {
    assert_eq!(st.generator.ceiling, Dbfs(-30.0));
    let labels: Vec<_> = st
        .outputs
        .iter()
        .map(|o| (o.channel, o.label.clone().unwrap()))
        .collect();
    assert_eq!(labels, [(0, "Main L".into()), (3, "Sub".into())]);
    assert!(st.rig.monitor_at_start);
    assert_eq!(st.rig.artnet_target, Some(s.target));
    assert_eq!(st.rig.artnet_fade, ArtnetFade::new(2.5).unwrap());
}

#[test]
fn a_current_file_loads_whole_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let s = seeded();
    let file = file_with_meters(&s, json!([meter_json(&s)]));
    std::fs::write(
        dir.path().join("rig.json"),
        serde_json::to_vec_pretty(&file).unwrap(),
    )
    .unwrap();

    let h = start("rig-file-current", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let st = c.state();
    assert!(
        st.rig.damaged_files.is_empty(),
        "{:?}",
        st.rig.damaged_files
    );
    assert_rig_kept(&st, &s);
    let m = st.measurements.iter().find(|m| m.id == MeasId(1)).unwrap();
    assert_eq!(m.config, s.meter, "limits and cue intact");
    drop(c);
    h.shutdown();
    let rj = rig_json(dir.path());
    assert_eq!(rj["version"], VERSION);
    assert!(rj["meters"][0].get("compared").is_none());
    let written = std::fs::read(dir.path().join("rig.json")).unwrap();

    // What it wrote, the next start reads unchanged and leaves unchanged.
    let h = start("rig-file-current-again", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let again = c.state();
    assert!(again.rig.damaged_files.is_empty());
    assert_rig_kept(&again, &s);
    let m = again
        .measurements
        .iter()
        .find(|m| m.id == MeasId(1))
        .unwrap();
    assert_eq!(m.config, s.meter);
    assert_eq!(m.config.scope, MeasScope::Rig);
    drop(c);
    h.shutdown();
    assert_eq!(std::fs::read(dir.path().join("rig.json")).unwrap(), written);
}

/// A file of an older version (as every build before the current one wrote) or a newer one
/// is set aside under its version and nothing is read from it: the rig starts as with no
/// file, the ceiling at the `--max-level` bound, never at a level read from a shape this
/// build does not know.
#[test]
fn another_version_is_set_aside_and_nothing_read() {
    let s = seeded();
    for v in [VERSION - 1, 1, VERSION + 1] {
        let dir = tempfile::tempdir().unwrap();
        let mut file = file_with_meters(&s, json!([meter_json(&s)]));
        file["version"] = json!(v);
        let bytes = serde_json::to_vec_pretty(&file).unwrap();
        std::fs::write(dir.path().join("rig.json"), &bytes).unwrap();

        let h = start(&format!("rig-file-v{v}"), dir.path());
        let (mut c, _sub) = connect(&h, &[]);
        let st = c.state();
        assert_ne!(st.generator.ceiling, Dbfs(-30.0), "version {v}");
        assert!(st.outputs.iter().all(|o| o.label.is_none()), "version {v}");
        assert!(st.measurements.is_empty(), "version {v}");
        assert!(!st.rig.monitor_at_start, "version {v}");
        assert_eq!(st.rig.artnet_target, None, "version {v}");
        assert_eq!(
            std::fs::read(dir.path().join(format!("rig.json.v{v}"))).unwrap(),
            bytes,
            "version {v}"
        );
        drop(c);
        h.shutdown();
    }
}

/// The current version's file holding `meters`: what of them can be read is, the rest of
/// the file always is.
fn write_with_meters(dir: &Path, s: &Seeded, meters: Value) -> Vec<u8> {
    let bytes = serde_json::to_vec_pretty(&file_with_meters(s, meters)).unwrap();
    std::fs::write(dir.join("rig.json"), &bytes).unwrap();
    bytes
}

fn damage(st: &State) -> &DamagedFile {
    assert_eq!(st.rig.damaged_files.len(), 1, "{:?}", st.rig.damaged_files);
    let d = &st.rig.damaged_files[0];
    assert_eq!(d.kind, StateFile::RigSettings);
    d
}

#[test]
fn unreadable_meters_keep_the_ceiling_monitor_and_artnet() {
    let s = seeded();
    let meter = meter_json(&s);

    // A meters section that is not a list: the meters are lost, nothing else.
    let dir = tempfile::tempdir().unwrap();
    let bytes = write_with_meters(dir.path(), &s, json!("garbage"));
    let h = start("rig-file-garbage", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let st = c.state();
    assert_rig_kept(&st, &s);
    assert!(st.measurements.is_empty());
    let d = damage(&st);
    assert!(d.reason.contains("meters:"), "{}", d.reason);
    assert!(d.reason.contains("started without meters"), "{}", d.reason);
    let aside = d.set_aside_as.clone().unwrap();
    assert_eq!(std::fs::read(aside).unwrap(), bytes);
    // Written again with what was read, every section there.
    assert_eq!(rig_json(dir.path())["meters"], json!([]));
    assert_eq!(rig_json(dir.path())["ceiling_dbfs"], json!(-30.0));
    drop(c);
    h.shutdown();

    // One meter of a shape this build does not know, and one whose layout it cannot read
    // (a pane type that changed): the first is lost, the second meters without its layout.
    let dir = tempfile::tempdir().unwrap();
    let mut bad_layout = meter.clone();
    bad_layout["layout"] = json!({ "a pane": "of another build" });
    let unknown = json!({ "id": 2, "config": { "kind": "a later kind" }, "running": true });
    write_with_meters(dir.path(), &s, json!([bad_layout, unknown]));
    let h = start("rig-file-meter", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let st = c.state();
    assert_rig_kept(&st, &s);
    let ids: Vec<_> = st.measurements.iter().map(|m| m.id).collect();
    assert_eq!(ids, [MeasId(1)]);
    assert_eq!(st.measurements[0].config, s.meter);
    assert_eq!(st.measurements[0].layout, None);
    let d = damage(&st);
    assert!(
        d.reason.contains("meters: meter 1: layout:"),
        "{}",
        d.reason
    );
    assert!(
        d.reason.contains("meters: meter 2: config:"),
        "{}",
        d.reason
    );
    drop(c);
    h.shutdown();
}
