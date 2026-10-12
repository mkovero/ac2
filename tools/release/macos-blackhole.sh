#!/usr/bin/env bash
# Runs the macOS binaries on the BlackHole 2ch virtual loopback driver (Core Audio through
# cpal): the device is listed, an in-process duplex self-test, and a daemon session recorded
# for a while with the delay finder running on it. CI installs BlackHole and sox and
# restarts coreaudiod first; docs/design/macos-ci-audio.md has the findings behind each check.
#
#   tools/release/macos-blackhole.sh target/debug virtual-audio-logs
#
# ac2 never emits: nothing here passes --emit or starts the generator. A capture check needs
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
    "$AC2D" --backend cpal --no-autosave --recordings "$WORK" > "$LOG/ac2d-$1.stdout.txt" 2> "$LOG/ac2d-$1.stderr.txt" &
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

echo "== ac2d --backend cpal: '$DEV' in the device list"
daemon_start devices
ac2j devices 15 devices
ID=$(jq -r --arg d "$DEV" '.[] | select(.kind == "cpal") | .devices[] | select(.name == $d and .input and .output) | .id' \
    "$LOG/devices.json" | head -1)
[[ -n "$ID" ]] || fail "ac2 devices does not list '$DEV' with inputs and outputs (see devices.json)"
echo "  $ID"
daemon_stop

echo "== ac2 selftest duplex on $ID, $BUFFER-frame buffer (silent outputs; sox plays a -40 dBFS sine)"
stimulus_start sine
tmo 60 "$AC2" --json selftest duplex --backend cpal --device "$ID" --in 1-2 --outputs 2 --duration 20s \
    --buffer "${BUFFER}samples" > "$LOG/selftest-duplex.json" 2> "$LOG/selftest-duplex.stderr.txt"
code=$?
stimulus_stop
[[ $code -eq 142 ]] && fail "selftest duplex did not finish in 60 s (a capture without microphone access hangs)"
jq -c '{pass, failures, opened: (.opened | {sample_rate, buffer_frames, clock, index}), peak: .capture.peak_dbfs,
    xruns: .capture.xrun_blocks, gaps: .capture.index_gaps, disc: .capture.discontinuity_blocks}' \
    "$LOG/selftest-duplex.json" 2> /dev/null || { cat "$LOG/selftest-duplex.stderr.txt" >&2; fail "selftest duplex exited $code with no report"; }
[[ $(jq -r .pass "$LOG/selftest-duplex.json") == true ]] ||
    fail "selftest duplex failed: $(jq -c .failures "$LOG/selftest-duplex.json")"
# The sine arrives unchanged on both inputs: BlackHole is a bit-exact loop.
jq -e '.capture.peak_dbfs | length == 2 and all(. > -41 and . < -39)' "$LOG/selftest-duplex.json" > /dev/null ||
    fail "captured peak $(jq -c .capture.peak_dbfs "$LOG/selftest-duplex.json") dBFS, expected the -40 dBFS sine on both inputs"
echo "  pass"

echo "== session on $ID, recording $SECONDS_REC s; delay finder on in 2 vs in 1 (sox delays in 2 by $DELAY samples)"
daemon_start session
stimulus_start pink
ac2j session-open 30 session open --backend cpal --device "$ID" --out-device "$ID" --in 1-2 --outputs 2 \
    --buffer "${BUFFER}samples"
rate=$(jq -r .open.sample_rate_hz "$LOG/session-open.json")
echo "  $rate Hz, buffer $(jq -r .open.buffer_frames "$LOG/session-open.json") frames, clock $(jq -r .open.clock "$LOG/session-open.json")"
ac2j rec-start 15 rec start --name blackhole --max "$((SECONDS_REC + 60))s"
ac2j meas-new 15 meas new tf --name bh --ref 1 --meas 2
ac2j meas-start 15 meas start bh
half=$((SECONDS_REC / 2))
sleep "$half"
ac2j delay-find 30 delay find bh
sleep "$((SECONDS_REC - half))"
ac2j session-status 15 session status
ac2j rec-stop 30 rec stop
ac2j session-close 15 session close
stimulus_stop
daemon_stop

problems=()
stopped=$(jq -c '.stopped // empty' "$LOG/session-status.json")
[[ -z "$stopped" ]] || problems+=("session audio stopped: $stopped")
frames=$(jq -r .frames "$LOG/rec-stop.json")
disc=$(jq -r .discontinuities "$LOG/rec-stop.json")
expected=$((rate * SECONDS_REC))
echo "  recorded $frames frames (expected about $expected), $disc discontinuities"
((frames * 10 >= expected * 9)) || problems+=("recorded $frames frames, expected at least 90 % of $expected")
[[ "$disc" == 0 ]] || problems+=("$disc discontinuities in the capture")
jq -c '.finding | {outcome: .outcome.type, first: .outcome.first.delay_samples, psr_db: .confidence.psr_db,
    uncertainty: .confidence.uncertainty_samples, reasons: .outcome.reasons}' "$LOG/delay-find.json"
outcome=$(jq -r .finding.outcome.type "$LOG/delay-find.json")
found=$(jq -r '.finding.outcome.first.delay_samples // empty' "$LOG/delay-find.json")
if [[ "$outcome" != accepted ]]; then
    problems+=("delay finder: $outcome, expected an accepted $DELAY-sample delay")
elif ! jq -en --argjson f "$found" --argjson d "$DELAY" '($f - $d) | (if . < 0 then -. else . end) <= 1' > /dev/null; then
    problems+=("delay finder read $found samples, expected $DELAY ± 1")
fi
if ((${#problems[@]})); then
    printf 'FAIL: %s\n' "${problems[@]}" >&2
    exit 1
fi
echo "== BlackHole ($DEV): pass"
