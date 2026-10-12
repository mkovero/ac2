#!/usr/bin/env bash
# macOS spike, GitHub-hosted runner ONLY: inspect and grant microphone (kTCCServiceMicrophone)
# rows in the runner's TCC databases. Never run this on a real machine.
#
#   tcc.sh dump
#   tcc.sh grant <client>…    # absolute paths of executables (client_type 1 = path)
#   tcc.sh revoke             # delete every microphone row
set -uo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_ENVIRONMENT:-}" != github-hosted ]]; then
    echo "tcc.sh edits TCC only on a GitHub-hosted runner" >&2
    exit 2
fi

DBS=("$HOME/Library/Application Support/com.apple.TCC/TCC.db" "/Library/Application Support/com.apple.TCC/TCC.db")

case "${1:-}" in
    dump)
        csrutil status
        for db in "${DBS[@]}"; do
            echo "== $db"
            sudo sqlite3 "$db" ".schema access"
            sudo sqlite3 -separator ' | ' "$db" "SELECT service, client, client_type, auth_value, auth_reason FROM access ORDER BY service, client"
        done
        ;;
    grant)
        shift
        now=$(date +%s)
        for c in "$@"; do
            for db in "${DBS[@]}"; do
                echo "grant kTCCServiceMicrophone to $c in $db"
                sudo sqlite3 "$db" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, flags, last_modified) VALUES ('kTCCServiceMicrophone', '$c', 1, 2, 4, 1, 0, $now);"
            done
        done
        # tccd caches decisions per process.
        sudo killall tccd 2>/dev/null
        killall tccd 2>/dev/null
        sleep 1
        ;;
    revoke)
        # Removes every microphone row: what a capture gets when nobody granted access.
        for db in "${DBS[@]}"; do
            sudo sqlite3 "$db" "DELETE FROM access WHERE service = 'kTCCServiceMicrophone';"
        done
        sudo killall tccd 2>/dev/null
        killall tccd 2>/dev/null
        sleep 1
        ;;
    *) echo "usage: tcc.sh dump | grant <client>… | revoke" >&2; exit 64 ;;
esac
