//! Builds a static libzmq 4.3.5 with CURVE, linked against the static libsodium that
//! `libsodium-sys-stable` provides (Unix: built from its bundled, signature-checked tarball;
//! Windows MSVC: its minisign-verified prebuilt archive).
//!
//! Optimisation: `cc` compiles libzmq at the opt-level cargo hands this build script, i.e. this
//! package's profile. As a workspace member, `ac2-zmq` does not get the
//! `[profile.dev.package."*"] opt-level = 2` that dependencies get, so a dev build would compile
//! libzmq (and its CURVE path) at -O0. `OPT_LEVEL` is therefore raised to at least
//! [`MIN_C_OPT_LEVEL`] before `cc` runs. Release builds keep cargo's level (3, "s" or "z").
//! Override with `AC2_ZMQ_C_OPT_LEVEL` (e.g. `0` to debug inside libzmq).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Lowest opt-level the C++ sources are compiled with in any profile.
const MIN_C_OPT_LEVEL: &str = "2";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AC2_ZMQ_C_OPT_LEVEL");

    let target = env::var("TARGET").expect("TARGET");
    let (sodium_include, sodium_lib) = sodium_location(&target);

    set_c_opt_level();

    let flags = env::var("CXXFLAGS").unwrap_or_default();
    let include = if target.contains("msvc") {
        // libsodium's headers mark every symbol `dllimport` under MSVC unless SODIUM_STATIC is
        // defined; the static archive is linked. cc reads CXXFLAGS when it compiles libzmq.
        set_env("CXXFLAGS", &format!("{flags} /DSODIUM_STATIC"));
        msvc_sodium_include_shim(&sodium_include)
    } else {
        // libzmq's sources are not -Wextra clean; cc would relay every warning as a cargo
        // warning on each build of this crate.
        set_env("CXXFLAGS", &format!("{flags} -w"));
        sodium_include
    };

    zeromq_src::Build::new()
        .with_libsodium(Some(zeromq_src::LibLocation::new(sodium_lib, include)))
        .build();

    if target.contains("windows") {
        // libzmq uses Winsock and the IP helper API; libsodium's randombytes uses RtlGenRandom
        // from advapi32. zeromq-src only names iphlpapi, and Rust's std does not guarantee the
        // others end up on the link line.
        for lib in ["ws2_32", "iphlpapi", "advapi32"] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}

fn set_env(key: &str, value: &str) {
    // SAFETY: a build script's main thread is the only thread at this point; nothing reads
    // the environment concurrently.
    unsafe { env::set_var(key, value) };
}

/// See the module docs: `cc` takes its optimisation level from `OPT_LEVEL`.
fn set_c_opt_level() {
    let level = match env::var("AC2_ZMQ_C_OPT_LEVEL") {
        Ok(explicit) => explicit,
        Err(_) => match env::var("OPT_LEVEL").as_deref() {
            Ok("3" | "s" | "z") => return,
            _ => MIN_C_OPT_LEVEL.to_owned(),
        },
    };
    set_env("OPT_LEVEL", &level);
}

/// Include and static-library directories of the libsodium that `libsodium-sys-stable` built.
///
/// From source (Unix) it exports both as `DEP_SODIUM_INCLUDE` / `DEP_SODIUM_LIB`. On MSVC it
/// unpacks the prebuilt zip into `<its OUT_DIR>/installed/libsodium/` but exports only
/// `DEP_SODIUM_INCLUDE = <its OUT_DIR>/installed/include` (a directory that does not exist)
/// and no lib dir; it still emits its own link-search path and `static=libsodium`. The real
/// layout is reconstructed from that anchor, picking the `static` (not `dynamic`/`ltcg`)
/// archive of the profile libsodium-sys-stable chose.
fn sodium_location(target: &str) -> (PathBuf, PathBuf) {
    let include = PathBuf::from(
        env::var("DEP_SODIUM_INCLUDE").expect("libsodium-sys-stable exports no include dir"),
    );
    if let Ok(lib) = env::var("DEP_SODIUM_LIB") {
        return (include, PathBuf::from(lib));
    }
    assert!(
        target.contains("msvc"),
        "libsodium-sys-stable exported no lib dir for {target}"
    );
    let installed = include
        .parent()
        .expect("DEP_SODIUM_INCLUDE has a parent")
        .join("libsodium");
    let arch = match env::var("CARGO_CFG_TARGET_ARCH")
        .expect("target arch")
        .as_str()
    {
        "x86_64" => "x64",
        "x86" => "Win32",
        "aarch64" => "ARM64",
        other => panic!("no prebuilt libsodium for MSVC arch {other}"),
    };
    let config = if env::var("PROFILE").expect("PROFILE") == "release" {
        "Release"
    } else {
        "Debug"
    };
    let include = installed.join("include");
    let lib = installed
        .join(arch)
        .join(config)
        .join("v143")
        .join("static");
    assert!(
        include.join("sodium.h").is_file() && lib.join("libsodium.lib").is_file(),
        "prebuilt libsodium not where expected: {} / {}",
        include.display(),
        lib.display()
    );
    (include, lib)
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
