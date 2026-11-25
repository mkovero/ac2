//! A session's SPL logs and trace data are untrusted like the rest of it: crafted ones
//! refuse the whole load before anything changes.
#![allow(clippy::unwrap_used)]

use crate::common;
use crate::sessions::{import, named, save, start, traces};

use std::path::Path;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode};
use common::*;

/// Copies session `from` to `to` (every file), edits its manifest, then writes `files`
/// (relative path, contents) over or beside the copied ones.
fn crafted_copy(
    dir: &Path,
    from: &str,
    to: &str,
    edit: impl Fn(&mut serde_json::Value),
    files: &[(&str, String)],
) {
    let (src, dst) = (
        dir.join("sessions").join(from),
        dir.join("sessions").join(to),
    );
    for sub in ["traces", "spl"] {
        std::fs::create_dir_all(dst.join(sub)).unwrap();
        if let Ok(rd) = std::fs::read_dir(src.join(sub)) {
            for e in rd {
                let e = e.unwrap();
                std::fs::copy(e.path(), dst.join(sub).join(e.file_name())).unwrap();
            }
        }
    }
    let text = std::fs::read_to_string(src.join("session.json")).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    edit(&mut v);
    std::fs::write(dst.join("session.json"), v.to_string()).unwrap();
    for (rel, contents) in files {
        std::fs::write(dst.join(rel), contents).unwrap();
    }
}

/// A session's SPL logs and trace data are untrusted like the rest of it: an SPL log of a
/// measurement the session does not hold or of one that is not an SPL meter, a log row the
/// meter could not have written, columns off the trace's grid or a magnitude past what the
/// math keeps finite each refuse the whole load (`invalid`) before anything changes. A NaN
/// magnitude is the protocol's "no value" and loads.
#[test]
fn a_session_with_crafted_logs_or_trace_data_refuses_to_load() {
    use ac2_proto::spl_log::{SplLogInfo, export_csv};
    let dir = tempfile::tempdir().unwrap();
    let h = start("load-crafted", dir.path(), true);
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    import(c, "rew_export.txt");
    c.ok(Command::MeasCreate {
        config: transfer("tf"),
    });
    c.ok(Command::MeasCreate {
        config: spl("FOH", 0),
    });
    save(c, "good", false).unwrap();
    let sdir = dir.path().join("sessions").join("good");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sdir.join("session.json")).unwrap()).unwrap();
    let id_of = |kind: &str| {
        manifest["measurements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["config"]["kind"]["type"] == kind)
            .unwrap_or_else(|| panic!("no {kind} in {manifest}"))["id"]
            .as_u64()
            .unwrap()
    };
    let (spl_id, tf_id) = (id_of("spl"), id_of("transfer"));
    let row = |k: u64, laeq: f64| SplLogRow {
        start: WallNs(1_790_000_000_000_000_000 + k * 1_000_000_000),
        measured: Seconds(1.0),
        laeq: Dbfs(laeq),
        lceq: Dbfs(-23.25),
        lzeq: Dbfs(-22.0),
        lcpeak: Dbfs(-5.5),
        lafmax: Dbfs(f64::NEG_INFINITY),
        sensitivity: None,
        position: None,
    };
    let log = |laeq: f64| {
        export_csv(
            &SplLogInfo {
                meas: MeasId(u32::try_from(spl_id).unwrap()),
                name: "FOH".into(),
                input: 0,
                mic: None,
            },
            &[row(0, -25.0), row(1, laeq)],
        )
    };
    let with_log = |meas: u64| {
        move |v: &mut serde_json::Value| {
            v["spl_logs"] =
                serde_json::json!([{ "meas": meas, "file": "spl/crafted.csv", "bands": null }]);
        }
    };
    let file = |s: String| [("spl/crafted.csv", s)];
    crafted_copy(
        dir.path(),
        "good",
        "spl-ok",
        with_log(spl_id),
        &file(log(-24.0)),
    );
    crafted_copy(
        dir.path(),
        "good",
        "spl-orphan",
        with_log(99),
        &file(log(-24.0)),
    );
    crafted_copy(
        dir.path(),
        "good",
        "spl-on-tf",
        with_log(tf_id),
        &file(log(-24.0)),
    );
    crafted_copy(
        dir.path(),
        "good",
        "spl-nan",
        with_log(spl_id),
        &file(log(f64::NAN)),
    );
    crafted_copy(
        dir.path(),
        "good",
        "spl-huge",
        with_log(spl_id),
        &file(log(1e300)),
    );

    // The trace's data file, with its first data row edited or dropped.
    let trace_file = manifest["traces"][0]["file"].as_str().unwrap().to_owned();
    let csv = std::fs::read_to_string(sdir.join(&trace_file)).unwrap();
    let edited = |f: &dyn Fn(&str) -> Option<String>| -> String {
        let mut first = true;
        let mut out = String::new();
        let mut in_data = false;
        for l in csv.lines() {
            if in_data && first && !l.starts_with('#') && !l.is_empty() {
                first = false;
                if let Some(r) = f(l) {
                    out.push_str(&r);
                    out.push('\n');
                }
                continue;
            }
            in_data |= l.starts_with("freq_hz,");
            out.push_str(l);
            out.push('\n');
        }
        out
    };
    let mag = |v: &'static str| {
        move |l: &str| {
            let mut f: Vec<&str> = l.split(',').collect();
            f[1] = v;
            Some(f.join(","))
        }
    };
    let keep = |_: &mut serde_json::Value| {};
    crafted_copy(
        dir.path(),
        "good",
        "trace-short",
        keep,
        &[(&trace_file, edited(&|_| None))],
    );
    crafted_copy(
        dir.path(),
        "good",
        "trace-huge",
        keep,
        &[(&trace_file, edited(&mag("1e30")))],
    );
    crafted_copy(
        dir.path(),
        "good",
        "trace-nan",
        keep,
        &[(&trace_file, edited(&mag("NaN")))],
    );

    let t = import(c, "smaart_tf.txt");
    let before = c.state();
    assert!(before.working.dirty);
    for name in [
        "spl-orphan",
        "spl-on-tf",
        "spl-nan",
        "spl-huge",
        "trace-short",
        "trace-huge",
    ] {
        let e = c
            .call(Command::FileLoad {
                session: named(name),
                discard: true,
            })
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{name}: {e:?}");
        let after = c.state();
        assert_eq!(after.measurements, before.measurements, "{name}");
        assert_eq!(after.traces, before.traces, "{name}");
        assert_eq!(after.spl_logs, before.spl_logs, "{name}");
        assert_eq!(after.working, before.working, "{name}");
    }
    assert!(traces(c).iter().any(|x| x.id == t.id));

    for name in ["trace-nan", "spl-ok"] {
        c.ok(Command::FileLoad {
            session: named(name),
            discard: true,
        });
    }
    let m = c.state().measurements;
    assert_eq!(m.len(), 2);
    h.shutdown();
}
