#!/usr/bin/env bash
# ac2's own ASIO backend under Wine on WineASIO, JACK dummy driver: cross-builds ac2d, ac2
# and the ac2-loop check for x86_64-pc-windows-msvc, then runs them.
#
# Usage: tools/wine-asio/ac2.sh        (needs the image; tools/wine-asio/run.sh builds it)
# Env:   RATE (48000), PERIOD (256), CACHE_DIR (/work/ac2-scratch/wine-asio), IMAGE,
#        SECONDS_REC (30), CYCLES (20)
# Output in tools/wine-asio/out/ac2/:
#   build.log            cross builds
#   selftest.json        ac2 selftest duplex --backend asio, SECONDS_REC s (silent, in process)
#   loop.log             ac2-loop: the device list, then CYCLES opens of WineASIO with
#                        out_1 -> in_1 connected inside JACK
#   bufsize.log          ac2-loop MODE=bufsize: jack_bufsize mid-stream, end, reopen
#   jackd.log
# The daemon (ac2d) is built but not run: see README.md, "ac2's backend".
#
# Audio safety: JACK runs the dummy driver (no sound device) and wineasio never connects to
# JACK's (dummy) system ports. The selftest and the bufsize run are silent: nothing passes
# --emit or starts the generator. Only ac2-loop writes nonzero samples (-20 dBFS single-sample
# impulses), it refuses any driver but WineASIO, and its output reaches only its own input
# through the connection made here.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
IMAGE="${IMAGE:-ac2-wine-asio:dev}"
RATE="${RATE:-48000}"
PERIOD="${PERIOD:-256}"
CACHE_DIR="${CACHE_DIR:-/work/ac2-scratch/wine-asio}"
out="$here/out/ac2"
mkdir -p "$out" "$CACHE_DIR/ac2-target" "$CACHE_DIR/registry" "$CACHE_DIR/git"

docker run --rm --memory 8g --ulimit memlock=-1 --cpus 4 --shm-size 256m \
    -e RATE="$RATE" -e PERIOD="$PERIOD" \
    -e RUSTUP_TOOLCHAIN=1.95.0 \
    -e SECONDS_REC="${SECONDS_REC:-30}" -e CYCLES="${CYCLES:-20}" \
    -v "$repo:/src:ro" \
    -v "$CACHE_DIR/ac2-target:/target" \
    -v "$CACHE_DIR/registry:/opt/cargo/registry" \
    -v "$CACHE_DIR/git:/opt/cargo/git" \
    -v "$out:/out" \
    "$IMAGE" bash -euo pipefail -c '
        # RUSTUP_TOOLCHAIN: the image toolchain (with the Windows targets), the same release
        # as rust-toolchain.toml, which would otherwise make rustup fetch one without them.
        # Each step returns on failure: set -e does not apply inside a function run in a list.
        build() {
            cd /src
            # The pinned prebuilt MSVC libsodium, as CI links it (see sodium-make).
            SODIUM_DIST_DIR=$(tools/release/fetch-libsodium.sh /target/libsodium-dist) || return 1
            export SODIUM_DIST_DIR
            mkdir -p /tmp/shim && cp /src/tools/wine-asio/sodium-make /tmp/shim/make
            export PATH=/tmp/shim:$PATH
            cargo xwin build --locked --target x86_64-pc-windows-msvc --target-dir /target \
                -p ac2d -p ac2-cli || return 1
            cd /src/tools/wine-asio/ac2-loop
            cargo xwin build --locked --release --target x86_64-pc-windows-msvc \
                --target-dir /target/ac2-loop
        }
        build > /out/build.log 2>&1 && echo "build ok" || { tail -40 /out/build.log; exit 1; }
        bin=/target/x86_64-pc-windows-msvc/debug
        loop=/target/ac2-loop/x86_64-pc-windows-msvc/release/wine-asio-ac2-loop.exe

        fail=0
        problem() { echo "PROBLEM: $*"; fail=1; }
        # wineasio prints a line to stdout when it loads; the JSON reply starts at the first line opening an object.
        ac2() { timeout 120 wine "$bin/ac2.exe" --json "$@" 2>>/out/ac2.stderr.log | tr -d "\r" | sed -n "/^{/,\$p"; }
        wait_for() {  # wait_for <seconds> <command...>: until the command succeeds
            local t=$1; shift
            for _ in $(seq $((t * 5))); do "$@" && return 0; sleep 0.2; done
            return 1
        }

        jackd --no-realtime -d dummy -r "$RATE" -p "$PERIOD" -C 2 -P 2 > /out/jackd.log 2>&1 &
        jack_pid=$!
        wait_for 10 jack_lsp >/dev/null 2>&1
        export WINEASIO_CLIENT_NAME=ac2

        echo "== ac2 selftest duplex --backend asio, ${SECONDS_REC} s (silent)"
        ac2 selftest duplex --backend asio --device "WineASIO Driver" --in 1-2 --outputs 2 \
            --duration "${SECONDS_REC}s" > /out/selftest.json || true
        jq -c "{pass, failures, opened: (.opened | {sample_rate, buffer_frames, clock, index, input_format})}" /out/selftest.json || problem "selftest: no report"
        [ "$(jq -r .pass /out/selftest.json)" = true ] || problem "selftest did not pass"

        echo "== ac2-loop: $CYCLES opens, impulses out_1 -> in_1 inside JACK"
        CYCLES=$CYCLES timeout 600 wine "$loop" > /out/loop.log 2>&1 &
        loop_pid=$!
        # Each open loads the driver again, so its JACK client and ports are new each cycle.
        while kill -0 "$loop_pid" 2>/dev/null; do
            jack_connect ac2:out_1 ac2:in_1 >/dev/null 2>&1 || true
            sleep 0.1
        done
        wait "$loop_pid" || problem "ac2-loop exit $?"
        sed -n "/^# summary/,\$p" /out/loop.log

        echo "== jack_bufsize $((PERIOD * 2)) mid-stream: the stream ends, a reopen takes the new size"
        MODE=bufsize timeout 120 wine "$loop" > /out/bufsize.log 2>&1 &
        bs_pid=$!
        wait_for 60 grep -q "^READY" /out/bufsize.log || problem "bufsize: never ready"
        sleep 2
        jack_bufsize $((PERIOD * 2)) >> /out/jackd.log 2>&1
        wait "$bs_pid" || problem "bufsize exit $?"
        grep -v "^READY" /out/bufsize.log | grep -E "^(opened|ended|reopened|after|PASS|FAIL)"
        kill "$jack_pid"; wait "$jack_pid" 2>/dev/null || true
        wineserver -k || true
        [ $fail = 0 ] && echo "== wine-asio ac2: pass" || echo "== wine-asio ac2: FAIL"
        exit $fail
    '
