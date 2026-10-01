//! Builds a static libzmq with CURVE enabled, linked against the libsodium that
//! `libsodium-sys-stable` builds (Unix: from its bundled, signature-checked tarball;
//! Windows: from its signature-checked prebuilt archive).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let sodium_include =
        PathBuf::from(env::var("DEP_SODIUM_INCLUDE").expect("libsodium-sys-stable include dir"));
    let sodium_lib =
        PathBuf::from(env::var("DEP_SODIUM_LIB").expect("libsodium-sys-stable lib dir"));
    let target = env::var("TARGET").expect("TARGET");

    let include = if target.contains("msvc") {
        // libsodium's headers mark every symbol `dllimport` under MSVC unless SODIUM_STATIC is
        // defined; we link the static archive. cc reads CXXFLAGS from the environment when it
        // compiles libzmq.
        // SAFETY: build scripts are single-threaded at this point.
        unsafe { env::set_var("CXXFLAGS", "/DSODIUM_STATIC") };
        msvc_sodium_include_shim(&sodium_include)
    } else {
        // libzmq's sources are not -Wextra clean; cc would relay every warning as a cargo
        // warning on each build of this crate.
        let flags = env::var("CXXFLAGS").unwrap_or_default();
        // SAFETY: build scripts are single-threaded at this point.
        unsafe { env::set_var("CXXFLAGS", format!("{flags} -w")) };
        sodium_include
    };

    zeromq_src::Build::new()
        .with_libsodium(Some(zeromq_src::LibLocation::new(sodium_lib, include)))
        .build();
}

/// zeromq-src (MSVC only) copies `<include>/../../../builds/msvc/version.h`, a path that exists
/// in a libsodium source tree but not in the prebuilt archive. Mirror the headers into a
/// directory with that shape.
fn msvc_sodium_include_shim(include: &Path) -> PathBuf {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("sodium-shim");
    let shim_include = out.join("a").join("b").join("include");
    copy_dir(include, &shim_include);
    let msvc = out.join("builds").join("msvc");
    fs::create_dir_all(&msvc).expect("create shim builds/msvc");
    fs::copy(
        include.join("sodium").join("version.h"),
        msvc.join("version.h"),
    )
    .expect("copy sodium version.h");
    shim_include
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create dir");
    for entry in fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            fs::copy(&path, &dest).expect("copy header");
        }
    }
}
