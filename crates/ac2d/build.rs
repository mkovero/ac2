//! Build id: `<package version>+<12-hex git commit>` (or `+unknown` outside a git checkout),
//! overridable with `AC2_BUILD_ID`. The daemon reports it in `welcome.server` as
//! `(build <id>)`; the rule is identical to ac2-cli's so `ac2 daemon status` can compare.

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

fn main() {
    println!("cargo:rerun-if-env-changed=AC2_BUILD_ID");
    println!("cargo:rerun-if-changed=build.rs");
    let id = match std::env::var("AC2_BUILD_ID") {
        Ok(id) if !id.is_empty() => id,
        _ => {
            if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
                println!(
                    "cargo:rerun-if-changed={}",
                    PathBuf::from(dir).join("HEAD").display()
                );
            }
            if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
                let common = PathBuf::from(common);
                println!(
                    "cargo:rerun-if-changed={}",
                    common.join("refs/heads").display()
                );
                println!(
                    "cargo:rerun-if-changed={}",
                    common.join("packed-refs").display()
                );
            }
            let sha = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
            let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
            format!("{version}+{sha}")
        }
    };
    println!("cargo:rustc-env=AC2_BUILD_ID={id}");
}
