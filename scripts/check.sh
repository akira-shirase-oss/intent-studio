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

step drills
scripts/drills.sh

step workspace-e2e
scripts/workspace-e2e.sh

step deploy
scripts/test-deploy.sh

printf '\ncheck ok\n'
