//! The UI computes no measurement values: the shipped binary must not even link the DSP or
//! audio crates. Checked on the resolved dependency graph (`cargo tree`, normal and build
//! edges), so a transitive path is caught too.

use std::collections::BTreeSet;
use std::process::Command;

const FORBIDDEN: [&str; 5] = ["ac2-core", "ac2-audio", "rustfft", "realfft", "ac2d"];

#[test]
fn ui_does_not_link_dsp() {
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
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo tree");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let names: BTreeSet<&str> = text
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert!(names.contains("ac2-ui"), "{text}");
    // The display math comes from the scene layer.
    assert!(names.contains("ac2-scene"));
    for f in FORBIDDEN {
        assert!(!names.contains(f), "ac2-ui depends on {f}");
    }
}
