#!/usr/bin/env bash
# Runs the macOS binaries on the BlackHole 2ch virtual loopback driver, once per Core Audio
# backend (cpal, then coreaudio, the HAL directly) so the two compare on one build: the
# device is listed, an in-process duplex self-test, and a daemon session recorded for a while
# with the delay finder running on it. Given the coreaudio-loop binary, it then checks the
# coreaudio backend's exact index with impulses (the one emitting step, see below) and, last,
# what a revoked microphone grant does. CI installs BlackHole and sox and restarts coreaudiod
# first; docs/design/macos-ci-audio.md and docs/design/coreaudio.md have the findings.
#
#   tools/release/macos-blackhole.sh target/debug virtual-audio-logs [coreaudio-loop binary]
#
# ac2 never emits: nothing here passes --emit or starts ac2's generator. coreaudio-loop
# plays -20 dBFS impulses into BlackHole under the same guards as sox. A capture check needs
# non-zero samples, so sox plays the stimulus into BlackHole: a -40 dBFS sine during the
# self-test, and pink noise on both channels with the second 480 samples late during the
# session, which the delay finder must read back. sox plays only on a GitHub-hosted runner
# (no audio hardware) and only to a device named BlackHole.
#
# Without the microphone grant a Core Audio capture blocks on a consent prompt nobody can
# answer instead of returning silence, so every ac2 call runs under a deadline.
set -uo pipefail

BIN=${1:?usage: macos-blackhole.sh <bin dir> <log dir>}
LOG=${2:?usage: macos-blackhole.sh <bin dir> <log dir>}
LOOP=${3:-}
DEV=${AC2_BLACKHOLE_DEVICE:-BlackHole 2ch}
SECONDS_REC=${AC2_BLACKHOLE_SECONDS:-30}
# 1024 frames lost whole cycles on the shared VM (Intel) and 512 on both architectures;
# a CI check must not flake on VM scheduling.
BUFFER=2048
DELAY=480

mkdir -p "$LOG"
LOG=$(cd "$LOG" && pwd)
AC2=$(cd "$BIN" && pwd)/ac2
AC2D=$(cd "$BIN" && pwd)/ac2d
WORK=$(mktemp -d)
STIM=""
DAEMON=""

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

# A command with a deadline; perl because macOS has no timeout(1). Exit 142 is the alarm.
tmo() {
    local s=$1
    shift
    perl -e 'alarm shift; exec @ARGV or die "exec $ARGV[0]: $!"' "$s" "$@"
}

stimulus_start() {
    if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_ENVIRONMENT:-}" != github-hosted ]]; then
        fail "the sox stimulus plays only on a GitHub-hosted runner"
    fi
    [[ "$DEV" == BlackHole* ]] || fail "the sox stimulus plays only into BlackHole, not '$DEV'"
    if [[ $1 == pink ]]; then
        sox -q -V1 -r 48000 -c 1 -n -t coreaudio "$DEV" synth 3600 pinknoise remix 1 1 delay 0 "${DELAY}s" gain -30 \
            2> "$LOG/sox-$1.stderr.txt" &
    else
        sox -q -V1 -n -t coreaudio "$DEV" synth 3600 sine 997 gain -40 2> "$LOG/sox-$1.stderr.txt" &
    fi
    STIM=$!
    sleep 1
    kill -0 "$STIM" 2> /dev/null || fail "sox could not play into '$DEV' (see sox-$1.stderr.txt)"
}

stimulus_stop() {
    if [[ -n "$STIM" ]]; then
        kill "$STIM" 2> /dev/null
        wait "$STIM" 2> /dev/null
        STIM=""
    fi
}

daemon_stop() {
    [[ -n "$DAEMON" ]] || return 0
    tmo 15 "$AC2" daemon stop > /dev/null 2>&1
    for _ in $(seq 20); do
        kill -0 "$DAEMON" 2> /dev/null || break
        sleep 0.5
    done
    kill -9 "$DAEMON" 2> /dev/null
    wait "$DAEMON" 2> /dev/null
    DAEMON=""
}

cleanup() {
    stimulus_stop
    daemon_stop
    rm -rf "$WORK"
}
trap cleanup EXIT

# One ac2 call with --json and a deadline: stdout to <log>/<name>.json, stderr to
# <name>.stderr.txt. Fails the script on a non-zero exit.
ac2j() {
    local name=$1 deadline=$2
    shift 2
    tmo "$deadline" "$AC2" --json "$@" > "$LOG/$name.json" 2> "$LOG/$name.stderr.txt"
    local code=$?
    if [[ $code -ne 0 ]]; then
        cat "$LOG/$name.stderr.txt" >&2
        [[ $code -eq 142 ]] && fail "ac2 $* did not finish in $deadline s (a capture without microphone access hangs)"
        fail "ac2 $* exited $code"
    fi
}

daemon_start() {
    "$AC2D" --backend "$BACKEND" --no-autosave --recordings "$WORK" > "$LOG/ac2d-$1.stdout.txt" 2> "$LOG/ac2d-$1.stderr.txt" &
    DAEMON=$!
    for _ in $(seq 120); do
        kill -0 "$DAEMON" 2> /dev/null || fail "ac2d exited at startup (see ac2d-$1.stderr.txt)"
        tmo 5 "$AC2" --json status > /dev/null 2>&1 && return 0
        sleep 0.5
    done
    fail "ac2d did not answer within 60 s"
}

echo "== Core Audio lists '$DEV'"
listed=""
for _ in $(seq 30); do
    if system_profiler SPAudioDataType 2> /dev/null | grep -q "^ *$DEV:"; then
        listed=1
        break
    fi
    sleep 1
done
system_profiler SPAudioDataType > "$LOG/system_profiler.txt" 2>&1
[[ -n "$listed" ]] || fail "'$DEV' is not listed by Core Audio (is BlackHole installed and coreaudiod restarted?)"

echo "== microphone grant"
# TCC attributes a CLI's capture to the process at the top of its chain, the runner's
# hosted-compute-agent; the image grants it the microphone. A missing row hangs capture.
agent=/opt/hca/hosted-compute-agent
query="SELECT auth_value FROM access WHERE service='kTCCServiceMicrophone' AND client='$agent'"
user_db="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
system_db="/Library/Application Support/com.apple.TCC/TCC.db"
grant=$(sqlite3 "$user_db" "$query" 2> /dev/null)
[[ "$grant" == 2 ]] || grant=$(sudo -n sqlite3 "$system_db" "$query" 2> /dev/null)
if [[ "$grant" != 2 ]]; then
    fail "no kTCCServiceMicrophone grant (auth_value 2) for $agent in the user or system TCC.db;
      capture would hang waiting for consent. docs/design/macos-ci-audio.md (Q2) has the row to insert."
fi
echo "  $agent: allowed"

problems=()
# One backend's pass; a failure goes to problems, prefixed by the backend, so the other
# backend still runs and both show up side by side.
run_backend() {
    BACKEND=$1
    local b=$1 code rate half stopped frames disc expected outcome found
    echo "== [$b] ac2d --backend $b: '$DEV' in the device list"
    daemon_start "$b-devices"
    ac2j "$b-devices" 15 devices
    ID=$(jq -r --arg d "$DEV" --arg b "$b" '.[] | select(.kind == $b) | .devices[] | select(.name == $d and .input and .output) | .id' \
        "$LOG/$b-devices.json" | head -1)
    daemon_stop
    if [[ -z "$ID" ]]; then
        problems+=("[$b] ac2 devices does not list '$DEV' with inputs and outputs (see $b-devices.json)")
        return
    fi
    jq -c --arg id "$ID" '.[] | .devices[] | select(.id == $id) | {id, duplex_clock, index, notes,
        rates: .input.rates_hz, buffers: .input.buffer_frames}' "$LOG/$b-devices.json"
    if [[ $b == coreaudio ]]; then
        [[ "$ID" == coreaudio:BlackHole2ch_UID ]] || problems+=("[$b] BlackHole listed as $ID")
        jq -e --arg id "$ID" '.[] | .devices[] | select(.id == $id) | .duplex_clock == "single_callback" and .index == "exact"' \
            "$LOG/$b-devices.json" > /dev/null || problems+=("[$b] BlackHole not listed as one callback with an exact index")
    fi

    echo "== [$b] ac2 selftest duplex on $ID, $BUFFER-frame buffer (silent outputs; sox plays a -40 dBFS sine)"
    stimulus_start sine
    tmo 60 "$AC2" --json selftest duplex --backend "$b" --device "$ID" --in 1-2 --outputs 2 --duration 20s \
        --buffer "${BUFFER}samples" > "$LOG/$b-selftest-duplex.json" 2> "$LOG/$b-selftest-duplex.stderr.txt"
    code=$?
    stimulus_stop
    if [[ $code -eq 142 ]]; then
        problems+=("[$b] selftest duplex did not finish in 60 s (a capture without microphone access hangs)")
    elif ! jq -c '{pass, failures, opened: (.opened | {sample_rate, buffer_frames, clock, index}), peak: .capture.peak_dbfs,
        xruns: .capture.xrun_blocks, gaps: .capture.index_gaps, disc: .capture.discontinuity_blocks}' \
        "$LOG/$b-selftest-duplex.json" 2> /dev/null; then
        cat "$LOG/$b-selftest-duplex.stderr.txt" >&2
        problems+=("[$b] selftest duplex exited $code with no report")
    elif [[ $(jq -r .pass "$LOG/$b-selftest-duplex.json") != true ]]; then
        problems+=("[$b] selftest duplex failed: $(jq -c .failures "$LOG/$b-selftest-duplex.json")")
    elif ! jq -e '.capture.peak_dbfs | length == 2 and all(. > -41 and . < -39)' "$LOG/$b-selftest-duplex.json" > /dev/null; then
        # The sine arrives unchanged on both inputs: BlackHole is a bit-exact loop.
        problems+=("[$b] captured peak $(jq -c .capture.peak_dbfs "$LOG/$b-selftest-duplex.json") dBFS, expected the -40 dBFS sine on both inputs")
    else
        echo "  pass"
    fi

    echo "== [$b] session on $ID, recording $SECONDS_REC s; delay finder on in 2 vs in 1 (sox delays in 2 by $DELAY samples)"
    daemon_start "$b-session"
    stimulus_start pink
    ac2j "$b-session-open" 30 session open --backend "$b" --device "$ID" --out-device "$ID" --in 1-2 --outputs 2 \
        --buffer "${BUFFER}samples"
    rate=$(jq -r .open.sample_rate_hz "$LOG/$b-session-open.json")
    echo "  $rate Hz, buffer $(jq -r .open.buffer_frames "$LOG/$b-session-open.json") frames, clock $(jq -r .open.clock "$LOG/$b-session-open.json")"
    ac2j "$b-rec-start" 15 rec start --name "blackhole-$b" --max "$((SECONDS_REC + 60))s"
    ac2j "$b-meas-new" 15 meas new tf --name bh --ref 1 --meas 2
    ac2j "$b-meas-start" 15 meas start bh
    half=$((SECONDS_REC / 2))
    sleep "$half"
    ac2j "$b-delay-find" 30 delay find bh
    sleep "$((SECONDS_REC - half))"
    ac2j "$b-session-status" 15 session status
    ac2j "$b-rec-stop" 30 rec stop
    ac2j "$b-session-close" 15 session close
    stimulus_stop
    daemon_stop

    stopped=$(jq -c '.stopped // empty' "$LOG/$b-session-status.json")
    [[ -z "$stopped" ]] || problems+=("[$b] session audio stopped: $stopped")
    frames=$(jq -r .frames "$LOG/$b-rec-stop.json")
    disc=$(jq -r .discontinuities "$LOG/$b-rec-stop.json")
    expected=$((rate * SECONDS_REC))
    echo "  recorded $frames frames (expected about $expected), $disc discontinuities"
    ((frames * 10 >= expected * 9)) || problems+=("[$b] recorded $frames frames, expected at least 90 % of $expected")
    [[ "$disc" == 0 ]] || problems+=("[$b] $disc discontinuities in the capture")
    jq -c '.finding | {outcome: .outcome.type, first: .outcome.first.delay_samples, psr_db: .confidence.psr_db,
        uncertainty: .confidence.uncertainty_samples, reasons: .outcome.reasons}' "$LOG/$b-delay-find.json"
    outcome=$(jq -r .finding.outcome.type "$LOG/$b-delay-find.json")
    found=$(jq -r '.finding.outcome.first.delay_samples // empty' "$LOG/$b-delay-find.json")
    if [[ "$outcome" != accepted ]]; then
        problems+=("[$b] delay finder: $outcome, expected an accepted $DELAY-sample delay")
    elif ! jq -en --argjson f "$found" --argjson d "$DELAY" '($f - $d) | (if . < 0 then -. else . end) <= 1' > /dev/null; then
        problems+=("[$b] delay finder read $found samples, expected $DELAY ± 1")
    fi
    echo "$b: selftest $(jq -c '.capture | {xruns: .xrun_blocks, gaps: .index_gaps, disc: .discontinuity_blocks}' \
        "$LOG/$b-selftest-duplex.json" 2> /dev/null), session $frames frames, $disc discontinuities, delay $found" \
        >> "$LOG/compare.txt"
}

: > "$LOG/compare.txt"
run_backend cpal
run_backend coreaudio
echo "== cpal and coreaudio side by side"
cat "$LOG/compare.txt"

if [[ -n "$LOOP" ]]; then
    echo "== [coreaudio] exact index: impulses out 1 -> in 1, each back at T + stated latency"
    # coreaudio-loop checks the same guards itself; this is the script's own refusal.
    [[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] ||
        fail "coreaudio-loop plays only on a GitHub-hosted runner"
    tmo 300 "$LOOP" loop > "$LOG/coreaudio-loop.txt" 2>&1
    code=$?
    grep -E '^(device|listed|RESULT|probe|PASS|FAIL)' "$LOG/coreaudio-loop.txt"
    [[ $code -eq 0 ]] || problems+=("[coreaudio] exact-index loop exited $code (see coreaudio-loop.txt)")
fi

if ((${#problems[@]})); then
    printf 'FAIL: %s\n' "${problems[@]}" >&2
    exit 1
fi

if [[ -n "$LOOP" ]]; then
    # Last, because it takes the runner's microphone grant away: what a request and an open
    # do when nobody has decided. The request runs on a thread of its own, bounded at 10 s;
    # the whole call is bounded again here in case the status query itself blocks.
    echo "== [coreaudio] microphone grant revoked: does the request return, does an open?"
    tccutil reset Microphone > /dev/null 2>&1
    sudo -n sqlite3 "$system_db" "DELETE FROM access WHERE service='kTCCServiceMicrophone'" 2> /dev/null
    sqlite3 "$user_db" "DELETE FROM access WHERE service='kTCCServiceMicrophone'" 2> /dev/null
    sudo -n killall tccd 2> /dev/null
    killall tccd 2> /dev/null
    sleep 2
    tmo 60 "$LOOP" consent > "$LOG/coreaudio-consent.txt" 2>&1
    code=$?
    cat "$LOG/coreaudio-consent.txt"
    [[ $code -eq 142 ]] && echo "  (the consent check itself did not finish in 60 s)"
fi
echo "== BlackHole ($DEV): pass"
