#!/usr/bin/env bash
# Runs what CI runs, in the same order. Run it before every push and check its exit code.
#
#   toolchain pin · fmt · clippy -D warnings · unit tests · architecture rules and the proof
#   that they can fail · cargo-deny (when installed; CI runs it in its own job)
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

step supply-chain
if command -v cargo-deny >/dev/null 2>&1; then
    cargo deny --locked check
else
    echo "cargo-deny not installed: skipped here (CI runs it in its own job)"
fi

printf '\ncheck ok\n'
