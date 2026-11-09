#!/usr/bin/env bash
# Phase A logger run: `ac2 selftest stamps` plus a side log of the host's time discipline.
#
#   run.sh <out-dir> <minutes> [ac2 selftest stamps arguments…]
#   run.sh runs/ketunkolo-96k-128 60 --backend jack --in 1
#
# Every 10 s, `chronyc tracking` goes to <out-dir>/chrony.log and, when it answers without
# root, `pmc -u -b 0 'GET TIME_STATUS_NP'` to <out-dir>/pmc.log; each entry starts with a
# `# <REALTIME ns>` line. Ctrl-C ends the logger early; it still closes its files.
# AC2 names the ac2 binary (default: `ac2` on PATH).
set -euo pipefail

if [ $# -lt 2 ]; then
    sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
out=$1
minutes=$2
shift 2
ac2=${AC2:-ac2}
mkdir -p "$out"

pmc_ok=0
if command -v pmc >/dev/null 2>&1 && timeout 5 pmc -u -b 0 'GET TIME_STATUS_NP' >/dev/null 2>&1; then
    pmc_ok=1
fi

side() {
    while :; do
        stamp="# $(date +%s%N)"
        if command -v chronyc >/dev/null 2>&1; then
            { echo "$stamp"; chronyc tracking 2>&1 || true; } >>"$out/chrony.log"
        fi
        if [ "$pmc_ok" = 1 ]; then
            { echo "$stamp"; timeout 5 pmc -u -b 0 'GET TIME_STATUS_NP' 2>&1 || true; } >>"$out/pmc.log"
        fi
        sleep 10
    done
}

side &
side_pid=$!
trap 'kill "$side_pid" 2>/dev/null || true' EXIT

# The logger sees Ctrl-C itself (same process group) and finishes its files; the script
# (a no-op trap, which children do not inherit, unlike an ignored signal) and tee (-i)
# outlive it so the summary still lands.
trap ':' INT
"$ac2" selftest stamps --minutes "$minutes" --out "$out" "$@" | tee -i "$out/summary.txt"
