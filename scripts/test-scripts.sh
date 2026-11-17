#!/usr/bin/env bash
# Rules for the repository's own scripts, run by check.sh:
#
#   S1  no script runs a binary from a hard-coded `target/` path. Cargo puts builds wherever
#       CARGO_TARGET_DIR (or a cargo config) says; a hard-coded path silently runs whatever stale
#       binary happens to sit in ./target (it once made a gate test an old build). Scripts ask
#       `cargo metadata` for the target directory instead.
set -euo pipefail
cd "$(dirname "$0")/.."

status=0
hits="$(grep -nE '(^|[^A-Za-z_$/{])target/(release|debug)/' scripts/*.sh | grep -v '^scripts/test-scripts.sh:' || true)"
if [ -n "$hits" ]; then
    echo "scripts: S1 a script runs a binary from a hard-coded target/ path:" >&2
    echo "$hits" >&2
    status=1
fi
[ "$status" = 0 ] && echo "scripts ok: S1 no hard-coded target/ paths"
exit "$status"
