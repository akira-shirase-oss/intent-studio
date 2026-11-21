#!/usr/bin/env bash
# The shipped binary must carry no trace of the mutation seams: INTENT_MUTATION, the
# feature name, every seam's env spelling — and the test clock's env var, route and
# body field — exist only under the `mutation-seams` cargo feature. A default release
# build must contain none of them. Its own step in CI (not hidden inside check.sh) so
# an earlier set -e failure can never skip it.
#
# Usage: scripts/check-seams.sh [binary]   — scan a binary (default: the one cargo builds)
#        scripts/check-seams.sh --prove    — build WITH the seams and require the scan to refuse
set -euo pipefail
cd "$(dirname "$0")/.."

pattern='INTENT_MUTATION|mutation-seams|no-session-check|no-session-delete|trust-any-forwarded|no-metrics-token|no-request-bound|no-content-type|no-egress-guard|no-lead-dedup|client-session-token|INTENT_TEST_CLOCK|__test/clock|advance_ms'

# scan <binary>: 0 when the binary exists and carries no seam, 1 otherwise. grep reads
# the file itself — `strings | grep -q` dies on SIGPIPE under pipefail exactly when it
# matches, and the pipeline's non-zero then inverts the verdict (B1). Only grep's own
# "no match" (exit 1) means absent; an error (exit 2+) is a failed check, not a pass.
scan() {
    local file="$1" status
    if [ ! -f "$file" ]; then
        echo "check-seams: no such binary: $file" >&2
        return 1
    fi
    status=0
    LC_ALL=C grep -qaE "$pattern" "$file" || status=$?
    case $status in
        0)
            echo "the binary carries a mutation seam" >&2
            return 1
            ;;
        1)
            return 0
            ;;
        *)
            echo "check-seams: grep failed on $file (exit $status)" >&2
            return 1
            ;;
    esac
}

if [ "${1:-}" = "--prove" ]; then
    # The check can go red: a feature build must fail the scan — and the proof must
    # find the binary where the build put it: a moved or absent path is a broken
    # proof, never a successful refusal (B1).
    scratch="$(mktemp -d)"
    trap 'rm -rf "$scratch"' EXIT
    CARGO_TARGET_DIR="$scratch" cargo build --release --locked -p intent-server --features mutation-seams >&2
    built="$scratch/release/intent-server"
    if [ ! -f "$built" ]; then
        echo "PROOF FAILED: the feature build produced no binary at $built" >&2
        exit 1
    fi
    if scan "$built" 2>/dev/null; then
        echo "PROOF FAILED: the scan passed a mutated binary" >&2
        exit 1
    fi
    echo "proof ok: the scan refuses a mutated binary"
    exit 0
fi

bin="${1:-}"
if [ -z "$bin" ]; then
    cargo build --release --locked -p intent-server
    # The binary just built, wherever CARGO_TARGET_DIR or a cargo config puts it (rule S1).
    bin="$(cargo metadata --format-version 1 --no-deps --locked | jq -r .target_directory)/release/intent-server"
fi
scan "$bin"
echo "seams absent from the shipped binary"
