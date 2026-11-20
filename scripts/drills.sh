#!/usr/bin/env bash
# The I0 drills against the real intent-server binary (release build, as deployed) and the local
# stack: intake behaviour, the studio API (I1), connection bounds, notify-down, kill -9 and db-down, each drill with
# the log-capture check.
# db-down stops the stack's Postgres, so it lives in a test binary of its own: Cargo runs test
# binaries one at a time (in name order, not the order given here), so nothing else is running
# while Postgres is down.
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/stack.sh up
# The asset worker is a binary of its own crate; the asset drill runs the real one.
cargo build --release --locked -p intent-worker
cargo test --release --locked -p intent-server \
    --no-fail-fast --test harness --test intake --test operator --test studio --test assets --test connections --test outbox_fencing --test notifier_pool --test readiness --test attacks --test drill_notify_down --test drill_kill --test drill_db_down \
    -- --ignored
