#!/usr/bin/env bash
# macOS spike: ac2 on the BlackHole virtual loopback driver on a GitHub-hosted runner.
#
#   probe.sh capture <tag> <bin> <logdir>   # who gets real samples: sox, the HAL probe, ac2 selftest
#   probe.sh session <bin> <logdir>         # daemon session, 30 s recording
#
# ac2 never emits: no --emit, no generator. A capture-permission check needs non-zero
# samples, so a -40 dBFS sine is played into BlackHole by sox, and only on a GitHub-hosted
# runner (no audio hardware; BlackHole is a virtual driver).
set -uo pipefail

DEV="BlackHole 2ch"
STIM=""

stimulus_start() {
    if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_ENVIRONMENT:-}" != github-hosted ]]; then
        echo "the stimulus plays only on a GitHub-hosted runner" >&2
        exit 2
    fi
    sox -q -V1 -n -t coreaudio "$DEV" synth 3600 sine 997 gain -40 &
    STIM=$!
    sleep 1
}

stimulus_stop() {
    [[ -n "$STIM" ]] && kill "$STIM" 2>/dev/null
    wait "$STIM" 2>/dev/null
    STIM=""
}
trap stimulus_stop EXIT

capture() {
    local tag=$1 bin=$2 log=$3/$1
    mkdir -p "$log"
    stimulus_start
    echo "== [$tag] sox records $DEV for 2 s"
    sox -V1 -t coreaudio "$DEV" -n trim 0 2 stat 2>&1 | grep -E 'Maximum amplitude|RMS +amplitude|Samples read' | tee "$log/sox-stat.txt"
    echo "== [$tag] HAL IOProc on $DEV, 3 s"
    "$bin/hal-timestamps" "$DEV" 3 2>&1 | tee "$log/hal.txt" | grep -E 'authorization|rate|latency|callbacks|gaps|Start'
    echo "== [$tag] ac2 selftest duplex on $DEV, 8 s (silent outputs; sox is the source)"
    "$bin/ac2" --json selftest duplex --backend cpal --device "$DEV" --in 1-2 --outputs 2 --duration 8s \
        > "$log/selftest.json" 2> "$log/selftest.stderr.txt"
    echo "  exit $?"
    jq -c '{pass, failures, opened, peak: .capture.peak_dbfs, blocks: .capture.blocks, gaps: .capture.index_gaps, disc: .capture.discontinuity_blocks, xruns: .capture.xrun_blocks, in_rate: .capture.rate, in_lag: .capture.input_lag, in_cb: .capture.callback_interval, out_rate: .output.rate, out_lead: .output.playback_lead, out_disc: .output.discontinuities}' "$log/selftest.json" 2>/dev/null || cat "$log/selftest.stderr.txt"
    stimulus_stop
}

ac2j() { # <name> args… : JSON reply to <log>/<name>.json, parsed reply on stdout
    local name=$1
    shift
    "$BIN/ac2" --json "$@" > "$LOG/$name.json" 2> "$LOG/$name.stderr.txt"
    local code=$?
    [[ $code -ne 0 ]] && { echo "ac2 $* exited $code" >&2; cat "$LOG/$name.stderr.txt" >&2; }
    cat "$LOG/$name.json"
    return $code
}

session() {
    BIN=$1 LOG=$2/session
    mkdir -p "$LOG"
    local work
    work=$(mktemp -d)
    "$BIN/ac2d" --backend cpal --no-autosave --recordings "$work" > "$LOG/ac2d.stdout.txt" 2> "$LOG/ac2d.stderr.txt" &
    local daemon=$!
    for _ in $(seq 120); do
        ac2j status status > /dev/null 2>&1 && break
        sleep 0.5
    done
    echo "== ac2 devices"
    ac2j devices devices | jq -c '.[] | select(.kind == "cpal") | .devices[] | {name, id, in: .input.max_channels, out: .output.max_channels}'
    local id
    id=$(jq -r --arg d "$DEV" '.[] | select(.kind == "cpal") | .devices[] | select(.name == $d and .input and .output) | .id' "$LOG/devices.json" | head -1)
    echo "  $DEV id: $id"
    stimulus_start
    echo "== session open"
    ac2j session-open session open --backend cpal --device "$id" --out-device "$id" --in 1-2 --outputs 2 | jq -c '.open'
    ac2j rec-start rec start --name blackhole --max 90s > /dev/null
    sleep 30
    ac2j session-status session status > /dev/null
    jq -c '{stopped}' "$LOG/session-status.json"
    echo "== rec stop"
    ac2j rec-stop rec stop | jq -c '{frames, discontinuities}'
    ac2j session-close session close > /dev/null
    stimulus_stop
    ac2j daemon-stop daemon stop > /dev/null
    wait "$daemon"
    echo "  ac2d exit $?"
    ls -l "$work"
    rm -rf "$work"
}

case "${1:-}" in
    capture) capture "$2" "$3" "$4" ;;
    session) session "$2" "$3" ;;
    *) echo "usage: probe.sh capture <tag> <bin> <logdir> | session <bin> <logdir>" >&2; exit 64 ;;
esac
