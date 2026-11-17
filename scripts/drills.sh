#!/usr/bin/env bash
# The I0 drills against the real intent-server binary (release build, as deployed) and the local
# stack: intake behaviour, connection bounds, notify-down, kill -9 and db-down, each drill with
# the log-capture check.
# db-down stops the stack's Postgres, so it runs alone, last (Cargo runs test binaries one at a
# time, in the order given).
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/stack.sh up
cargo test --release --locked -p intent-server \
    --test intake --test operator --test connections --test outbox_fencing --test notifier_pool --test readiness --test drill_notify_down --test drill_kill --test drill_db_down \
    -- --ignored
