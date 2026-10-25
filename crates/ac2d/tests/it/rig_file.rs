//! The rig settings file (`rig.json`) across builds: a file of the previous format version
//! (rig meters kept as session measurements, before they gained compare picks and display
//! offsets) loads whole; a meters section that cannot be read costs the meters only, never
//! the ceiling, the outputs, the monitor at start or the Art-Net cues; and what a daemon
//! writes, the next one reads unchanged.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::net::UdpSocket;
use std::path::Path;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ReplyBody};
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

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
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

/// A rig settings file as the build before rig meters had their own type wrote it: format
/// version 1, the meter a session measurement (with `delay`, without `compared` and
/// `offset`).
fn v1_file(s: &Seeded) -> Value {
    json!({
        "format": "ac2-rig-settings",
        "version": 1,
        "ceiling_dbfs": -30.0,
        "outputs": [{ "channel": 0, "label": "Main L" }, { "channel": 3, "label": "Sub" }],
        "measurements": [{
            "id": 1,
            "config": s.meter,
            "running": true,
            "delay": null,
            "colour": null,
            "layout": null
        }],
        "monitor_at_start": true,
        "monitor_session": s.monitor,
        "artnet_target": s.target,
        "artnet_fade": 2.5
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
fn a_v1_file_loads_whole_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let s = seeded();
    let v1 = serde_json::to_vec_pretty(&v1_file(&s)).unwrap();
    std::fs::write(dir.path().join("rig.json"), &v1).unwrap();

    let h = start("rig-file-v1", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let st = state(&mut c);
    assert!(
        st.rig.damaged_files.is_empty(),
        "{:?}",
        st.rig.damaged_files
    );
    assert_rig_kept(&st, &s);
    let m = st.measurements.iter().find(|m| m.id == MeasId(1)).unwrap();
    assert_eq!(m.config, s.meter, "limits and cue intact");
    // The version 1 file is kept beside the new one.
    assert_eq!(std::fs::read(dir.path().join("rig.json.v1")).unwrap(), v1);
    let rj = rig_json(dir.path());
    assert_eq!(rj["version"], 4);
    assert_eq!(
        rj["monitor_session"],
        serde_json::to_value(&s.monitor).unwrap()
    );
    assert!(rj["meters"][0].get("compared").is_none());
    let written = std::fs::read(dir.path().join("rig.json")).unwrap();
    drop(c);
    h.shutdown();

    // What it wrote, the next start reads unchanged and leaves unchanged.
    let h = start("rig-file-v1-again", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let again = state(&mut c);
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

/// The meters `meters` (a version 2 file otherwise as seeded): what of them can be read is,
/// the rest of the file always is.
fn v2_with_meters(dir: &Path, s: &Seeded, meters: Value) -> Vec<u8> {
    let mut v = v1_file(s);
    let o = v.as_object_mut().unwrap();
    o.remove("measurements");
    o.insert("version".into(), json!(2));
    o.insert("osc_target".into(), Value::Null);
    o.insert("meters".into(), meters);
    let bytes = serde_json::to_vec_pretty(&v).unwrap();
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
    let meter = json!({
        "id": 1,
        "config": s.meter,
        "running": true,
        "colour": null,
        "layout": null
    });

    // A meters section that is not a list: the meters are lost, nothing else.
    let dir = tempfile::tempdir().unwrap();
    let bytes = v2_with_meters(dir.path(), &s, json!("garbage"));
    let h = start("rig-file-garbage", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let st = state(&mut c);
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
    v2_with_meters(dir.path(), &s, json!([bad_layout, unknown]));
    let h = start("rig-file-meter", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let st = state(&mut c);
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

/// A version 2 meter's own layout names a picked measurement as `meas`: it reads whole, the
/// pick as today's `content`, and the file is not set aside.
#[test]
fn a_v2_meter_layout_with_a_pick_reads_whole() {
    let s = seeded();
    let pane = |meas: Value| {
        json!({ "type": "pane", "pane": {
            "kind": "spl", "transfer_view": "coherence", "spectrum_view": "spectrum",
            "spl_view": "leq", "sweep_view": "response", "chrome": "no_grid", "meas": meas,
            "levels": { "transfer": null, "spectrum_dbfs": null, "spectrum_spl": null,
                        "distortion": null } } })
    };
    let meter = |id: u64, meas: Value| {
        json!({ "id": id, "config": s.meter, "running": true, "colour": null,
                "layout": pane(meas) })
    };
    let dir = tempfile::tempdir().unwrap();
    v2_with_meters(dir.path(), &s, json!([meter(1, json!(1))]));
    let h = start("rig-file-v2-layout", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let st = state(&mut c);
    assert!(
        st.rig.damaged_files.is_empty(),
        "{:?}",
        st.rig.damaged_files
    );
    assert_rig_kept(&st, &s);
    let panes = |id| {
        let m = st.measurements.iter().find(|m| m.id == MeasId(id)).unwrap();
        m.layout.as_ref().expect("the meter's layout").panes()
    };
    let picked = panes(1);
    assert_eq!(picked[0].chrome, layout::PlotChrome::NoGrid);
    assert_eq!(
        picked[0].content,
        Some(layout::PaneContent::Meas { meas: MeasId(1) })
    );
    let rj = rig_json(dir.path());
    assert_eq!(rj["version"], 4);
    assert_eq!(
        rj["meters"][0]["layout"]["pane"]["content"],
        json!({ "type": "meas", "meas": 1 })
    );
    drop(c);
    h.shutdown();
}

/// A version 3 meter's layout holds three plates, none for the spectrum's peak list: it
/// reads whole, the dragged plates kept and the peak list where the client puts it, and the
/// file is written as version 4.
#[test]
fn a_v3_meter_layout_with_three_plates_reads_whole() {
    let s = seeded();
    let auto = json!({ "at": null, "scale": 1.0 });
    let layout = json!({ "type": "pane", "pane": {
        "kind": "spectrum", "transfer_view": "coherence", "spectrum_view": "spectrum",
        "spl_view": "leq", "sweep_view": "response", "chrome": "full", "content": null,
        "levels": { "transfer": null, "spectrum_dbfs": null, "spectrum_spl": null,
                    "distortion": null },
        "plates": { "legend": auto, "readout": { "at": [0.0, 1.0], "scale": 2.0 },
                    "mark": auto } } });
    let meter = json!({ "id": 1, "config": s.meter, "running": true, "colour": null,
                        "layout": layout });
    let dir = tempfile::tempdir().unwrap();
    let mut v: Value =
        serde_json::from_slice(&v2_with_meters(dir.path(), &s, json!([meter]))).unwrap();
    v["version"] = json!(3);
    v.as_object_mut().unwrap().insert(
        "levels".into(),
        serde_json::to_value(LevelsSettings::default()).unwrap(),
    );
    std::fs::write(dir.path().join("rig.json"), serde_json::to_vec(&v).unwrap()).unwrap();
    let h = start("rig-file-v3-plates", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let st = state(&mut c);
    assert!(
        st.rig.damaged_files.is_empty(),
        "{:?}",
        st.rig.damaged_files
    );
    let m = &st.measurements[0];
    let pane = &m.layout.as_ref().expect("the meter's layout").panes()[0];
    assert_eq!(pane.plates.readout.at, Some([0.0, 1.0]));
    assert_eq!(pane.plates.peaks, layout::LayoutPlate::default());
    let rj = rig_json(dir.path());
    assert_eq!(rj["version"], 4);
    assert_eq!(rj["meters"][0]["layout"]["pane"]["plates"]["peaks"], auto);
    drop(c);
    h.shutdown();
}
