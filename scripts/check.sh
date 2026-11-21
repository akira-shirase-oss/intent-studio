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
# Linux-only cfg (the worker's seccomp confinement; the render crate's browser
# jail and its proof suite) never compiles under macOS clippy; the check-only
# target pass needs no linker. Scoped to crates without C dependencies — a
# full-workspace pass would build ring via rustls, which needs a cross C
# toolchain most hosts lack. The intent-render pass builds --all-targets under
# mutation-seams so the confinement PROOF file compiles too (CI 44891348696
# found it had shipped uncompilable). Skipped where the target is not
# installed — CI runs the workspace-wide pass on Linux, where every cfg
# compiles natively.
if rustup target list --installed 2>/dev/null | grep -qx "x86_64-unknown-linux-gnu"; then
    step "clippy (intent-worker, x86_64-unknown-linux-gnu)"
    cargo clippy -p intent-worker --all-targets --locked --target x86_64-unknown-linux-gnu -- -D warnings
    step "clippy (intent-render + confinement proofs, x86_64-unknown-linux-gnu)"
    cargo clippy -p intent-render --all-targets --locked --features mutation-seams \
        --target x86_64-unknown-linux-gnu -- -D warnings
fi

step tests
# --no-fail-fast: a red unit test must never hide what the other suites would have shown.
cargo test --workspace --locked --no-fail-fast

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
scripts/check-seams.sh

step drills
scripts/drills.sh

step workspace-e2e
scripts/workspace-e2e.sh

step deploy
scripts/test-deploy.sh

printf '\ncheck ok\n'
