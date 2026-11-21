#!/usr/bin/env bash
# The shipped binary must carry no trace of the mutation seams: INTENT_MUTATION, the
# feature name, and every seam's env spelling exist only under the `mutation-seams`
# cargo feature — a default release build must contain none of them. Its own step in
# CI (not hidden inside check.sh) so an earlier set -e failure can never skip it.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release --locked -p intent-server
# The binary just built, wherever CARGO_TARGET_DIR or a cargo config puts it (rule S1).
bin="$(cargo metadata --format-version 1 --no-deps --locked | jq -r .target_directory)/release/intent-server"
if strings "$bin" | grep -qE 'INTENT_MUTATION|mutation-seams|no-session-check|no-session-delete|trust-any-forwarded|no-metrics-token|no-request-bound|no-content-type|no-egress-guard|no-lead-dedup|client-session-token'; then
    echo "the shipped binary carries a mutation seam — the feature leaked into a default build" >&2
    exit 1
fi
echo "seams absent from the shipped binary"
