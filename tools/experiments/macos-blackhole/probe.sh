#!/usr/bin/env bash
# macOS spike: ac2 on the BlackHole virtual loopback driver on a GitHub-hosted runner.
#
#   probe.sh capture <tag> <bin> <logdir>   # who gets real samples: sox, the HAL probe, ac2 selftest
#   probe.sh hal <bin> <logdir>             # Core Audio IOProc and cpal timestamps, click loopbacks
#   probe.sh session <bin> <logdir>         # daemon session: 30 s recording, delay finder
#
# ac2 never emits: no --emit, no generator. A capture-permission check needs non-zero
# samples, so a -40 dBFS sine is played into BlackHole by sox, and only on a GitHub-hosted
# runner (no audio hardware; BlackHole is a virtual driver). For the delay finder sox plays
# pink noise on both channels, the second 480 samples late.
set -uo pipefail

DEV="BlackHole 2ch"
STIM=""

stimulus_start() {
    if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_ENVIRONMENT:-}" != github-hosted ]]; then
        echo "the stimulus plays only on a GitHub-hosted runner" >&2
        exit 2
    fi
    if [[ "${1:-sine}" == pink ]]; then
        sox -q -V1 -r 48000 -c 1 -n -t coreaudio "$DEV" synth 3600 pinknoise remix 1 1 delay 0 480s gain -30 &
    else
        sox -q -V1 -n -t coreaudio "$DEV" synth 3600 sine 997 gain -40 &
    fi
    STIM=$!
    sleep 1
}

stimulus_stop() {
    [[ -n "$STIM" ]] && kill "$STIM" 2>/dev/null
    wait "$STIM" 2>/dev/null
    STIM=""
}
trap stimulus_stop EXIT

# Runs a command with a deadline: a capture without microphone access can block for good.
tmo() {
    local s=$1
    shift
    perl -e 'alarm shift; exec @ARGV' "$s" "$@"
}

summary() {
    jq -c '{pass, failures, opened, peak: .capture.peak_dbfs, blocks: .capture.blocks, gaps: .capture.index_gaps, disc: .capture.discontinuity_blocks, xruns: .capture.xrun_blocks, in_rate: .capture.rate, in_lag: .capture.input_lag, in_cb: .capture.callback_interval, out_rate: .output.rate, out_lead: .output.playback_lead, out_disc: .output.discontinuities}' "$1" 2>/dev/null || cat "${1%.json}.stderr.txt"
}

capture() {
    local tag=$1 bin=$2 log=$3/$1
    mkdir -p "$log"
    stimulus_start
    echo "== [$tag] sox records $DEV for 2 s"
    tmo 15 sox -V1 -t coreaudio "$DEV" -n trim 0 2 stat 2>&1 | grep -E 'Maximum amplitude|RMS +amplitude|Samples read' | tee "$log/sox-stat.txt"
    echo "== [$tag] HAL IOProc on $DEV, 3 s"
    tmo 20 "$bin/hal-timestamps" "$DEV" 3 2>&1 | tee "$log/hal.txt" | grep -E 'authorization|rate|latency|callbacks|gaps|Start'
    echo "== [$tag] ac2 selftest duplex on $DEV, 8 s (silent outputs; sox is the source)"
    tmo 40 "$bin/ac2" --json selftest duplex --backend cpal --device "$DEV" --in 1-2 --outputs 2 --duration 8s \
        > "$log/selftest.json" 2> "$log/selftest.stderr.txt"
    echo "  exit $?"
    summary "$log/selftest.json"
    [[ "$tag" == 0-pre ]] || { stimulus_stop; return; }
    for b in 512 2048; do
        echo "== [$tag] ac2 selftest duplex, $b-frame buffer, 8 s"
        tmo 40 "$bin/ac2" --json selftest duplex --backend cpal --device "$DEV" --in 1-2 --outputs 2 --duration 8s --buffer "${b}samples" \
            > "$log/selftest-$b.json" 2> "$log/selftest-$b.stderr.txt"
        echo "  exit $?"
        summary "$log/selftest-$b.json"
    done
    stimulus_stop
}

hal() {
    local bin=$1 log=$2/hal
    mkdir -p "$log"
    for b in 1024 512; do
        echo "== HAL IOProc, $b-frame buffer, click loopback, 6 s"
        "$bin/hal-timestamps" "$DEV" 6 "$b" click 2>&1 | tee "$log/hal-$b.txt" | grep -v -E '^device |^[0-9]+ '
    done
    for b in 1024 2048; do
        echo "== cpal timestamps, $b-frame buffer, click loopback, 6 s"
        "$bin/cpal-ts" "$DEV" 6 "$b" click 2>&1 | tee "$log/cpal-$b.txt"
    done
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

wav_stat() { # <wav> <channel>
    sox "$1" -n remix "$2" stat 2>&1 | grep -E 'Length|Maximum amplitude|RMS +amplitude'
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
    stimulus_start pink
    echo "== session open"
    ac2j session-open session open --backend cpal --device "$id" --out-device "$id" --in 1-2 --outputs 2 | jq -c '.open'
    ac2j rec-start rec start --name blackhole --max 90s > /dev/null
    echo "== tf (ref in 1, meas in 2; sox delays in 2 by 480 samples) and the delay finder"
    ac2j meas-new meas new tf --name bh --ref 1 --meas 2 | jq -c . | cut -c1-400
    ac2j meas-start meas start bh | jq -c . | cut -c1-400
    sleep 10
    ac2j delay-find delay find bh | jq -c . | cut -c1-1500
    sleep 18
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
    echo "-- recording ch1"
    wav_stat "$work/blackhole.wav" 1
    echo "-- recording ch2"
    wav_stat "$work/blackhole.wav" 2
    rm -rf "$work"
}

case "${1:-}" in
    capture) capture "$2" "$3" "$4" ;;
    hal) hal "$2" "$3" ;;
    session) session "$2" "$3" ;;
    *) echo "usage: probe.sh capture <tag> <bin> <logdir> | hal <bin> <logdir> | session <bin> <logdir>" >&2; exit 64 ;;
esac
