#!/usr/bin/env bash
# Copy the suite to the rig, run it there in a terminal (audible stages ask for Enter),
# fetch the run directory, analyse it here and compare it with the stored baselines.
#   ./rig-run.sh [--rig rigs/pupu.toml] <crosscheck run flags...>
# e.g. ./rig-run.sh --emit -50dbfs --emit-speaker -50dbfs
set -euo pipefail
cd "$(dirname "$0")"
RIG=rigs/pupu.toml
if [[ "${1:-}" == "--rig" ]]; then RIG=$2; shift 2; fi
PY_LOCAL=${PYTHON:-python3}
if [[ -z "${CROSSCHECK_RUNS:-}" && -d /work/ac2-crosscheck/runs ]]; then CROSSCHECK_RUNS=/work/ac2-crosscheck/runs; fi
RUNS=${CROSSCHECK_RUNS:-$PWD/runs}
SUITE=$(git describe --always --dirty --abbrev=7 2>/dev/null || echo unknown)
read -r SSH WORKDIR PY < <("$PY_LOCAL" -c 'import sys,tomllib;r=tomllib.load(open(sys.argv[1],"rb"))["rig"];print(r["ssh"],r["workdir"],r["python"])' "$RIG")
TS=$(date -u +%Y%m%dT%H%M%SZ)
ssh "$SSH" "mkdir -p $WORKDIR"
rsync -a --delete --exclude runs/ --exclude baselines/ --exclude __pycache__/ --exclude .pytest_cache/ ./ "$SSH:$WORKDIR/"
rc=0
ssh -t "$SSH" "cd $WORKDIR && CROSSCHECK_SUITE_COMMIT=$SUITE $PY -m crosscheck run --rig $RIG --out runs/$TS $(printf '%q ' "$@")" || rc=$?
mkdir -p "$RUNS/$TS"
rsync -a "$SSH:$WORKDIR/runs/$TS/" "$RUNS/$TS/" || true
if [[ -f "$RUNS/$TS/manifest.json" ]]; then
  "$PY_LOCAL" -m crosscheck analyse "$RUNS/$TS" || true
  echo "report: $RUNS/$TS/report/report.md"
  # differences are for the operator to read, not a failed run
  "$PY_LOCAL" -m crosscheck compare "$RUNS/$TS" || true
fi
exit $rc
