#!/usr/bin/env bash
# Runs what CI runs, in the same order. Run it before every push and check its exit code.
#
#   toolchain pin · fmt · clippy -D warnings · unit tests · architecture rules and the proof
#   that they can fail · cargo-deny (when installed; CI runs it in its own job) · the workspace
#   app: svelte-check, the UI kit's tests, the build · the I0 drills against the real binary and
#   the local stack (needs Docker) · the workspace in Chromium against the real server · the
#   production path: the release package started from the env example, backup and restore,
#   the systemd unit and the Caddyfile
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$*"; }

step toolchain
scripts/check-toolchain.sh

step fmt
cargo fmt --all --check

step clippy
cargo clippy --workspace --all-targets --locked -- -D warnings
# Linux-only cfg (the worker's seccomp confinement; the server's non-dumpable setup and
# probe) never compiles under macOS clippy; the check-only target pass needs no linker.
# Scoped to intent-worker — all hand-rolled Linux code lives there (the server's items are
# safe rustix calls), and a full-workspace pass would build C deps (ring via rustls) that
# need a cross C toolchain most hosts lack. Skipped where the target is not installed —
# CI runs the workspace-wide pass on Linux, where every cfg compiles natively.
if rustup target list --installed 2>/dev/null | grep -qx "x86_64-unknown-linux-gnu"; then
    step "clippy (intent-worker, x86_64-unknown-linux-gnu)"
    cargo clippy -p intent-worker --all-targets --locked --target x86_64-unknown-linux-gnu -- -D warnings
fi

step tests
cargo test --workspace --locked

step architecture
scripts/check-architecture.sh
scripts/test-architecture-check.sh
scripts/test-scripts.sh

step supply-chain
if command -v cargo-deny >/dev/null 2>&1; then
    deny_log="$(mktemp)"
    if ! cargo deny --locked check 2>"$deny_log"; then
        if grep -q 'failed to fetch advisory database' "$deny_log"; then
            # Offline (the advisory database could not be refreshed): check against the cached
            # copy and say so. CI always fetches a fresh one.
            echo "NOTICE: advisory database unreachable; checking against the cached copy" >&2
            cargo deny --locked --offline check
        else
            cat "$deny_log" >&2
            rm -f "$deny_log"
            exit 1
        fi
    fi
    rm -f "$deny_log"
else
    echo "cargo-deny not installed: skipped here (CI runs it in its own job)"
fi

step workspace
[ -d node_modules ] || npm ci
npm run check
npm run test --workspace @intent-studio/ui
npm run test --workspace @intent-studio/workspace
npm run build --workspace @intent-studio/workspace
scripts/check-bundle.sh

step "mutation seams absent from the shipped binary"
# INTENT_MUTATION and its seams exist only under the `mutation-seams` cargo feature — a
# default release build must carry no trace of them (the enum, the parse, the seam names).
cargo build --release --locked -p intent-server
if strings target/release/intent-server | grep -qE 'INTENT_MUTATION|mutation-seams|no-session-check|no-session-delete|trust-any-forwarded|no-metrics-token|no-request-bound|no-content-type|no-egress-guard|no-lead-dedup'; then
    echo "the shipped binary carries a mutation seam — the feature leaked into a default build" >&2
    exit 1
fi

step drills
scripts/drills.sh

step workspace-e2e
scripts/workspace-e2e.sh

step deploy
scripts/test-deploy.sh

printf '\ncheck ok\n'
