#!/usr/bin/env bash
# Copy the suite to the rig, run it there in a terminal (audible stages ask for Enter),
# fetch the run directory and analyse it here.
#   ./rig-run.sh [--rig rigs/pupu.toml] <crosscheck run flags...>
# e.g. ./rig-run.sh --emit -50dbfs --emit-speaker -50dbfs
set -euo pipefail
cd "$(dirname "$0")"
RIG=rigs/pupu.toml
if [[ "${1:-}" == "--rig" ]]; then RIG=$2; shift 2; fi
PY_LOCAL=${PYTHON:-python3}
RUNS=${CROSSCHECK_RUNS:-$PWD/runs}
read -r SSH WORKDIR PY < <("$PY_LOCAL" -c 'import sys,tomllib;r=tomllib.load(open(sys.argv[1],"rb"))["rig"];print(r["ssh"],r["workdir"],r["python"])' "$RIG")
TS=$(date -u +%Y%m%dT%H%M%SZ)
ssh "$SSH" "mkdir -p $WORKDIR"
rsync -a --delete --exclude runs/ --exclude __pycache__/ --exclude .pytest_cache/ ./ "$SSH:$WORKDIR/"
rc=0
ssh -t "$SSH" "cd $WORKDIR && $PY -m crosscheck run --rig $RIG --out runs/$TS $(printf '%q ' "$@")" || rc=$?
mkdir -p "$RUNS/$TS"
rsync -a "$SSH:$WORKDIR/runs/$TS/" "$RUNS/$TS/" || true
if [[ -f "$RUNS/$TS/manifest.json" ]]; then
  "$PY_LOCAL" -m crosscheck analyse "$RUNS/$TS" || true
  echo "report: $RUNS/$TS/report/report.md"
fi
exit $rc
