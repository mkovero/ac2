//! Build id: `<package version>+<12-hex git commit>` (or `+unknown` outside a git checkout),
//! overridable with `AC2_BUILD_ID`. `ac2 daemon status` compares it with the id the daemon
//! reports, so a stale daemon is detected by build, never by file times.

use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// Cargo treats a missing `rerun-if-changed` path as always stale, which would rerun this script
/// and rebuild the crate (and every test binary on top of it) on each cargo invocation;
/// `packed-refs`, for one, is absent in fresh clones and worktrees until git first packs refs.
fn rerun_if_exists(path: PathBuf) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=AC2_BUILD_ID");
    println!("cargo:rerun-if-changed=build.rs");
    let id = match std::env::var("AC2_BUILD_ID") {
        Ok(id) if !id.is_empty() => id,
        _ => {
            if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
                rerun_if_exists(PathBuf::from(dir).join("HEAD"));
            }
            if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
                let common = PathBuf::from(common);
                rerun_if_exists(common.join("refs/heads"));
                rerun_if_exists(common.join("packed-refs"));
            }
            let sha = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
            let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
            format!("{version}+{sha}")
        }
    };
    println!("cargo:rustc-env=AC2_BUILD_ID={id}");
}
