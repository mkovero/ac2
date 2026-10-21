#!/usr/bin/env bash
# Fails if any Rust source under crates/ is over the line limit (default 1000).
#
#   tools/release/loc-check.sh [limit]
#
# Why: an agent reads a whole file to change one part of it, and what it read stays in its
# context for the rest of the session; files past the limit get split (pure moves) instead.
set -euo pipefail
limit=${1:-1000}
cd "$(dirname "$0")/../.."
over=$(find crates -name '*.rs' -print0 | xargs -0 wc -l | awk -v n="$limit" '$2 != "total" && $1 > n')
if [ -n "$over" ]; then
    echo "Rust files over $limit lines (split them):"
    echo "$over"
    exit 1
fi
