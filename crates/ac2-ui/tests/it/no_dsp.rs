//! The UI computes no measurement values. Checked on the resolved dependency graph
//! (`cargo tree`, normal and build edges), so a transitive path is caught too:
//!
//! - Without the `embedded` feature the binary does not even link the DSP, audio or
//!   daemon crates.
//! - With it (the default), `ac2d` is the only way in: the daemon hosted in-process links
//!   them, but the UI itself depends on none of them directly, so its code cannot use them.

use std::collections::BTreeSet;
use std::process::Command;

const DSP: [&str; 4] = ["ac2-core", "ac2-audio", "rustfft", "realfft"];

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
    let names = tree(&["--no-default-features"]);
    // The display math comes from the scene layer.
    assert!(names.contains("ac2-scene"));
    for f in DSP.iter().chain(&["ac2d"]) {
        assert!(
            !names.contains(*f),
            "ac2-ui without `embedded` depends on {f}"
        );
    }
}

#[test]
fn only_the_embedded_daemon_brings_dsp_in() {
    let direct = tree(&["--depth", "1"]);
    assert!(
        direct.contains("ac2d"),
        "the default build embeds the daemon"
    );
    for f in DSP {
        assert!(!direct.contains(f), "ac2-ui depends on {f} directly");
    }
}
