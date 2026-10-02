#!/usr/bin/env bash
# Times a clean install of the Linux tarball to a first transfer-function measurement on the
# simulated rig, without touching the real user's files: everything lives in a temp HOME.
#
#   tools/release/first-measurement.sh ac2-<version>-linux-x86_64.tar.gz
#
# Steps are the ones in docs/install.md (CLI path; the app shows the same measurement).
set -euo pipefail

tarball=$(realpath "$1")
t=$(mktemp -d)
trap 'kill "${daemon:-0}" 2>/dev/null || true; rm -rf "$t"' EXIT
export HOME="$t/home" XDG_CONFIG_HOME="$t/home/.config" AC2_CONFIG_DIR="$t/home/.config/ac2" AC2_RUNTIME_DIR="$t/run"
mkdir -p "$HOME"
start=$(date +%s.%N)
step() { awk -v s="$start" -v n="$(date +%s.%N)" -v m="$*" 'BEGIN { printf "%6.2f s  %s\n", n - s, m }'; }

cd "$t"
tar xzf "$tarball"
cd ac2-*-linux-x86_64
./install.sh > /dev/null
bin="$HOME/.local/bin"
step "unpacked and installed"

"$bin/ac2d" --backend fake > "$t/ac2d.log" 2>&1 &
daemon=$!
for _ in $(seq 100); do [ -S "$AC2_RUNTIME_DIR/ctrl.sock" ] && break; sleep 0.05; done
"$bin/ac2" status > /dev/null
step "daemon up"

"$bin/ac2" session open --backend fake --in 1-2 > /dev/null
"$bin/ac2" meas new tf --ref 1 --meas 2 --name demo > /dev/null
"$bin/ac2" meas start demo > /dev/null
step "session open, transfer measurement running"

# Arm, fire (an empty line is Enter when stdin is not a terminal), stop after 6 s.
{ echo; sleep 6; echo q; } | "$bin/ac2" gen pink --out 1 --level -30dbfs > /dev/null &
gen=$!
sleep 4
"$bin/ac2" delay find demo --insert
"$bin/ac2" trace capture demo --name first > /dev/null
wait "$gen"
step "delay found and inserted; first trace captured"
"$bin/ac2" trace show first
"$bin/ac2" daemon stop > /dev/null
