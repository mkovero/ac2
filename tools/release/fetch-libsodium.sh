#!/usr/bin/env bash
# Vendors the prebuilt MSVC libsodium for Windows builds into a directory and prints it, for
# `SODIUM_DIST_DIR`:
#
#   export SODIUM_DIST_DIR=$(tools/release/fetch-libsodium.sh target/libsodium-dist)
#
# Why: on MSVC `libsodium-sys-stable` cannot run libsodium's configure script and instead
# downloads `libsodium-1.0.22-stable-msvc.zip` from download.libsodium.org at build time.
# That "-stable" archive is rebuilt in place whenever the stable branch moves, so two builds
# of the same commit can link different libsodium bytes. Release and CI builds instead use
# the immutable 1.0.22-RELEASE archive from the GitHub release, pinned here by SHA-256 and
# saved under the file name the crate expects. The crate's build script still checks the
# minisign signature against the libsodium release key, so the pin adds reproducibility on
# top of the authenticity check, it does not replace it.
set -euo pipefail

dir=${1:?usage: fetch-libsodium.sh <dir>}
base=https://github.com/jedisct1/libsodium/releases/download/1.0.22-RELEASE
zip_sha=3e03a726fac4bc09cb61d8f29d658ef7a5eca0811de59082130414f7ca2e4279
mkdir -p "$dir"
zip="$dir/libsodium-1.0.22-stable-msvc.zip"
sig="$zip.minisig"

check() { echo "$zip_sha  $zip" | sha256sum -c --quiet - >/dev/null 2>&1; }
if ! check; then
    curl -fsSL --retry 5 -o "$zip" "$base/libsodium-1.0.22-msvc.zip"
    curl -fsSL --retry 5 -o "$sig" "$base/libsodium-1.0.22-msvc.zip.minisig"
    check || { echo "libsodium archive does not match the pinned SHA-256" >&2; exit 1; }
fi
[ -f "$sig" ] || curl -fsSL --retry 5 -o "$sig" "$base/libsodium-1.0.22-msvc.zip.minisig"

# With SODIUM_DIST_DIR set, the build script reads its first choice, the source tarball it
# bundles (LATEST.tar.gz, which cannot be configured with MSVC), from that directory too
# before falling back to the zip. Copy it from the crate as published on crates.io.
crate_ver=1.24.0
cargo fetch --locked -q >&2
src=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/libsodium-sys-stable-$crate_ver | head -n1)
cp "$src/LATEST.tar.gz" "$src/LATEST.tar.gz.minisig" "$dir/"
cd "$dir" && pwd -W 2>/dev/null || pwd
