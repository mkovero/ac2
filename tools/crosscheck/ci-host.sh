#!/usr/bin/env bash
# The host-only crosscheck stages as CI runs them (README, "CI and releases"): the digital DUT
# matrix on a JACK dummy server of its own and an ac2d of its own, each level analysed and
# compared with baselines/host/. Exits non-zero when a status got worse, a judged value moved
# beyond its step, or a baseline check of a measured case is missing. The build hash is
# reported, never compared.
#
#   ci-host.sh <bin dir> <out dir> [--cases a,b] [--levels -10,-30]
#
# <bin dir> holds ac2, ac2d and ac2-jack-dut. PYTHON (default python3) needs numpy and
# JACK-Client. The dummy driver's playback ports lead nowhere: nothing reaches hardware.
set -euo pipefail

usage() { echo "usage: $0 <bin dir> <out dir> [--cases a,b] [--levels -10,-30]" >&2; exit 2; }
[ $# -ge 2 ] || usage
bin=$(cd "$1" && pwd)
out=$(mkdir -p "$2" && cd "$2" && pwd)
shift 2
cases=""
levels="-10,-30"
while [ $# -gt 0 ]; do
  case "$1" in
    --cases) cases=$2; shift 2 ;;
    --levels) levels=$2; shift 2 ;;
    *) usage ;;
  esac
done
for b in ac2 ac2d ac2-jack-dut; do
  [ -x "$bin/$b" ] || { echo "no $bin/$b" >&2; exit 2; }
done
here=$(cd "$(dirname "$0")" && pwd)
py=${PYTHON:-python3}
export PYTHONUNBUFFERED=1  # the run log is tee-d: keep stage lines in order with the binaries' stderr

# Everything the daemon and the clients touch lives under $out: not the desktop's server,
# config or session.
export JACK_DEFAULT_SERVER="ac2-ci-$$"
export HOME="$out/home" XDG_CONFIG_HOME="$out/home/.config" XDG_DATA_HOME="$out/home/.local/share"
export XDG_STATE_HOME="$out/home/.local/state" AC2_CONFIG_DIR="$out/home/.config/ac2"
# The runtime dir holds ac2d's ipc socket, whose path must fit sockaddr_un (108 bytes).
rt=$(mktemp -d "${TMPDIR:-/tmp}/ac2ci.XXXXXX")
export AC2_RUNTIME_DIR="$rt" XDG_RUNTIME_DIR="$rt"
export AC2_JACK_DUT="$bin/ac2-jack-dut" PATH="$bin:$PATH"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME"

pids=()
# shellcheck disable=SC2329  # run by the EXIT trap
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
  for p in "${pids[@]}"; do wait "$p" 2>/dev/null || true; done
  rm -rf "$rt"
}
trap cleanup EXIT
trap 'exit 143' TERM INT

# Without real-time scheduling a short period xruns: on a host under parallel builds 4096
# still did, 8192 did not, and matched the baselines (made at 2048) within their steps (README,
# "On a dummy server"). JACK_PERIOD overrides it; -w is the period in µs at 96 kHz.
period=${JACK_PERIOD:-8192}
jackd -n "$JACK_DEFAULT_SERVER" -d dummy -r 96000 -p "$period" -w $(( period * 1000000 / 96000 )) -C 8 -P 8 \
  >"$out/jackd.log" 2>&1 &
pids+=($!)
jack_up() { "$py" -c 'import jack; jack.Client("ac2-ci-probe", no_start_server=True).close()' >/dev/null 2>&1; }
for _ in $(seq 100); do
  jack_up && break
  sleep 0.1
done
jack_up || { cat "$out/jackd.log" >&2; echo "jackd did not start" >&2; exit 2; }

ac2d --max-level -10 --no-autosave >"$out/ac2d.log" 2>&1 &
pids+=($!)
for _ in $(seq 100); do
  [ -S "$AC2_RUNTIME_DIR/ctrl.sock" ] && break
  sleep 0.1
done
[ -S "$AC2_RUNTIME_DIR/ctrl.sock" ] || { cat "$out/ac2d.log" >&2; echo "ac2d did not start" >&2; exit 2; }

cd "$here"
rc=0
IFS=, read -ra lv <<<"$levels"
[ -n "$cases" ] || cases=$("$py" -c 'import sys, tomllib; print(",".join(c["name"] for c in tomllib.load(open(sys.argv[1], "rb"))["dut"]["cases"]))' rigs/host.toml)
IFS=, read -ra cs <<<"$cases"
# A dummy server without real-time scheduling can xrun on a shared runner. The suite then
# discards the take (or the DUT's own count turns WARN), which is the host's timing, not
# ac2's measurement: such a case is measured again, at most `attempts` times. Each case is a
# run of its own so that a retry repeats one case, not the matrix; each compares its own
# checks (the matrix-wide dut.coverage.* rows pool those same checks).
attempts=3
xruns() {  # <run dir> <run log>: did a take or the DUT see an xrun?
  grep -q 'xrun(s) during the take' "$2" && return 0
  "$py" - "$1/report/results.json" <<'EOF'
import json, sys
r = json.load(open(sys.argv[1]))
sys.exit(0 if any(c["id"].endswith(".dut.xruns") and (c.get("value") or 0) > 0 for c in r["checks"]) else 1)
EOF
}
for l in "${lv[@]}"; do
  for c in "${cs[@]}"; do
    t0=$(date +%s)
    for a in $(seq "$attempts"); do
      run="$out/runs/${c}${l}dbfs-$a"
      log="$out/run-${c}${l}-$a.log"
      st=0
      "$py" -m crosscheck run --rig rigs/host.toml --stages dut --cases "$c" --emit "${l}dbfs" \
        --skip "rew,ac2_tf" --yes --out "$run" 2>&1 | tee "$log" || st=$?
      [ "$st" -eq 0 ] || { echo "$c at ${l} dBFS: run exited $st" >&2; break; }
      # A baseline may hold a reviewed FAIL (baseline --force): analyse's own exit code is
      # not the gate, compare's is.
      "$py" -m crosscheck analyse "$run" --no-plots || true
      xruns "$run" "$log" && [ "$a" -lt "$attempts" ] && { echo "$c at ${l} dBFS: xruns, again"; continue; }
      break
    done
    [ "$st" -eq 0 ] && { "$py" -m crosscheck compare "$run" --strict || st=1; }
    [ "$st" -eq 0 ] || rc=1
    echo "$c at ${l} dBFS: $(( $(date +%s) - t0 )) s, attempt $a, $([ "$st" -eq 0 ] && echo ok || echo FAILED)"
  done
done
exit $rc
