//! The UI computes no measurement values. Checked on the resolved dependency graph
//! (`cargo tree`, normal and build edges), so a transitive path is caught too: the app does
//! not even link the DSP, audio or daemon crates; it starts `ac2d` as a process.

use std::collections::BTreeSet;
use std::process::Command;

/// The DSP, audio and room-simulation crates: the simulator's results reach the UI as
/// `State.sim`, never by computing them.
const DSP: [&str; 5] = ["ac2-core", "ac2-audio", "ac2-roomsim", "rustfft", "realfft"];

fn tree(extra: &[&str]) -> BTreeSet<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "tree",
            "--locked",
            "-p",
            "ac2-ui",
            "-e",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .args(extra)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo tree");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let names: BTreeSet<String> = text
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    assert!(names.contains("ac2-ui"), "{text}");
    names
}

#[test]
fn ui_does_not_link_dsp() {
    let names = tree(&[]);
    // The display math comes from the scene layer.
    assert!(names.contains("ac2-scene"));
    for f in DSP.iter().chain(&["ac2d"]) {
        assert!(!names.contains(*f), "ac2-ui depends on {f}");
    }
}
