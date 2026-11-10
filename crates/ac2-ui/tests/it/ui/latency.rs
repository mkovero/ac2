//! `--latency-log`: a frame applied and drawn yields a row whose stages run in order.

use super::*;

/// The CSV's lines as column → value maps, comments skipped.
fn rows(text: &str) -> Vec<std::collections::HashMap<String, String>> {
    let mut lines = text.lines().filter(|l| !l.starts_with('#'));
    let Some(head) = lines.next() else {
        return Vec::new();
    };
    let names: Vec<&str> = head.split(',').collect();
    assert_eq!(names, ac2_ui::latency::COLUMNS);
    lines
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            assert_eq!(f.len(), names.len(), "{l}");
            names
                .iter()
                .zip(f)
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect()
        })
        .collect()
}

#[test]
fn latency_rows_in_stage_order() {
    if !have_gpu("latency_rows_in_stage_order") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("latency.csv");
    let log = ac2_ui::latency::Latency::create(&path, &["test run".into()]).expect("log");
    h.state_mut().set_latency_log(log);
    // Passes drawn and rendered (the probe runs in the render) while new frames arrive.
    for _ in 0..30 {
        h.step();
        h.render().expect("render");
        std::thread::sleep(Duration::from_millis(20));
    }
    h.state_mut()
        .set_latency_log(ac2_ui::latency::Latency::off());
    let text = std::fs::read_to_string(&path).expect("read log");
    assert!(text.contains("# test run"), "{text}");
    let rows = rows(&text);
    assert!(!rows.is_empty());
    let num = |r: &std::collections::HashMap<String, String>, c: &str| -> Option<i128> {
        r[c].parse().ok()
    };
    // A spectrum pane's row whose frame was applied while logging, with its probe stamps.
    let full: Vec<_> = rows
        .iter()
        .filter(|r| r["view"].starts_with("spectrum") || r["view"] == "spectrograph")
        .filter(|r| num(r, "apply_ns").is_some() && num(r, "submit_ns").is_some())
        .collect();
    assert!(!full.is_empty(), "no complete spectrum row:\n{text}");
    for r in full {
        let mono: Vec<i128> = [
            "read_ns",
            "snapshot_ns",
            "apply_ns",
            "scene_ns",
            "prepare_ns",
            "submit_ns",
        ]
        .iter()
        .map(|c| num(r, c).unwrap_or_else(|| panic!("{c} in {r:?}")))
        .collect();
        assert!(mono.windows(2).all(|w| w[0] <= w[1]), "{mono:?} in {r:?}");
        let wall: Vec<i128> = ["capture_wall_ns", "read_wall_ns", "submit_wall_ns"]
            .iter()
            .map(|c| num(r, c).unwrap_or_else(|| panic!("{c} in {r:?}")))
            .collect();
        assert!(wall.windows(2).all(|w| w[0] <= w[1]), "{wall:?} in {r:?}");
        assert!(r["panes_drawn"].contains("spectrum"), "{r:?}");
        let lag: f64 = r["glide_lag_ms"].parse().expect("glide lag");
        assert!(lag >= 0.0, "{r:?}");
    }
}
