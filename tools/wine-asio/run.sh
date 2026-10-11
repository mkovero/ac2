#!/usr/bin/env bash
# Build the wine-asio image, then in a container: start JACK with the dummy driver, build the
# probes for x86_64-pc-windows-msvc, and run them under Wine on the WineASIO driver.
#
# Usage: tools/wine-asio/run.sh [--no-build]
# Env:   RATE (48000), PERIOD (256), CACHE_DIR (/work/ac2-scratch/wine-asio), IMAGE
# Output in tools/wine-asio/out/:
#   build.log        cross builds (both probes; azo_probe again without any SDK and for -gnu)
#   cpal.log         cpal_probe: cpal 0.18 ASIO host (Steinberg SDK via asio-sys), silence
#   azo.log          azo_probe: 2 in + 2 out, silence, 2 stop/start cycles
#   azo-plain.log    azo_probe with kAsioSupportsTimeInfo refused (plain bufferSwitch)
#   azo-loop-N.log   azo_probe, 3 separate processes, out_1 -> in_1 connected inside JACK
#   jackd.log
#
# Audio safety: JACK runs the dummy driver (no sound device) and wineasio is configured not
# to connect to JACK's system ports. Only the loopback runs write a nonzero sample (-20 dBFS
# impulses), and only into a JACK connection from the probe's own output to its own input.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="${IMAGE:-ac2-wine-asio:dev}"
RATE="${RATE:-48000}"
PERIOD="${PERIOD:-256}"
CACHE_DIR="${CACHE_DIR:-/work/ac2-scratch/wine-asio}"
out="$here/out"
mkdir -p "$out" "$CACHE_DIR/target"

if [[ "${1:-}" != "--no-build" ]]; then
    start=$(date +%s)
    docker build -t "$IMAGE" "$here"
    echo "image build: $(( $(date +%s) - start )) s, size $(docker image inspect "$IMAGE" --format '{{.Size}}' | awk '{printf "%.2f GB", $1/1e9}')"
fi

docker run --rm --memory 8g --ulimit memlock=-1 --cpus 4 --shm-size 256m \
    -e RATE="$RATE" -e PERIOD="$PERIOD" \
    -v "$here/probe:/src" \
    -v "$CACHE_DIR/target:/target" \
    -v "$out:/out" \
    "$IMAGE" bash -euo pipefail -c '
        cd /src
        {
            cargo xwin build --release --offline --target x86_64-pc-windows-msvc --target-dir /target
            echo "== azo_probe without cpal/asio-sys and without CPAL_ASIO_DIR"
            env -u CPAL_ASIO_DIR cargo tree --offline --target x86_64-pc-windows-msvc \
                --no-default-features -e normal | grep -E "asio|azo" || true
            env -u CPAL_ASIO_DIR cargo xwin build --release --offline --no-default-features \
                --bin azo_probe --target x86_64-pc-windows-msvc --target-dir /target/no-sdk
            echo "== azo_probe for x86_64-pc-windows-gnu (mingw)"
            env -u CPAL_ASIO_DIR cargo build --release --offline --no-default-features \
                --bin azo_probe --target x86_64-pc-windows-gnu --target-dir /target \
                && echo "gnu build ok" || echo "gnu build FAILED"
        } > /out/build.log 2>&1 || { tail -40 /out/build.log; exit 1; }
        bin=/target/x86_64-pc-windows-msvc/release

        # The dummy driver keeps time with the system clock and touches no hardware.
        jackd --no-realtime -d dummy -r "$RATE" -p "$PERIOD" -C 2 -P 2 > /out/jackd.log 2>&1 &
        jack_pid=$!
        for _ in $(seq 50); do jack_lsp >/dev/null 2>&1 && break; sleep 0.1; done
        export WINEASIO_CLIENT_NAME=probe

        status=0
        timeout 60 wine "$bin/cpal_probe.exe" > /out/cpal.log 2>&1 || { echo "cpal_probe exit $?"; status=1; }
        PROBE_CYCLES=2 timeout 60 wine "$bin/azo_probe.exe" > /out/azo.log 2>&1 \
            || { echo "azo_probe exit $?"; status=1; }
        PROBE_PLAIN=1 PROBE_SECONDS=2 timeout 60 wine "$bin/azo_probe.exe" > /out/azo-plain.log 2>&1 \
            || { echo "azo_probe plain exit $?"; status=1; }

        for n in 1 2 3; do
            PROBE_IMPULSE=1 PROBE_SECONDS=4 timeout 60 wine "$bin/azo_probe.exe" > "/out/azo-loop-$n.log" 2>&1 &
            probe_pid=$!
            for _ in $(seq 100); do
                jack_lsp 2>/dev/null | grep -q "^probe:in_1$" && break; sleep 0.05
            done
            jack_connect probe:out_1 probe:in_1 || echo "jack_connect failed (run $n)"
            jack_lsp -c >> /out/jackd.log 2>&1 || true
            wait "$probe_pid" || { echo "azo loop $n exit $?"; status=1; }
        done

        kill "$jack_pid"; wait "$jack_pid" 2>/dev/null || true
        wineserver -k || true
        exit $status
    '
echo "logs: $out"
for f in cpal azo azo-plain azo-loop-1 azo-loop-2 azo-loop-3; do
    echo "== $f"
    sed -n '/^# summary/,$p' "$out/$f.log"
done
