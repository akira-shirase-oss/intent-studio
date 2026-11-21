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
# The database layer's ignored tests run on the live stack — N15's class lives there
# and nothing else exercises it.
cargo test --release --locked -p intent-db --no-fail-fast -- --ignored
cargo test --release --locked -p intent-server \
    --no-fail-fast --test harness --test intake --test operator --test studio --test assets --test connections --test outbox_fencing --test notifier_pool --test readiness --test drill_notify_down --test drill_kill --test drill_db_down \
    -- --ignored
# The attacks run against the default build — what ships — first: every guard must
# refuse when INTENT_MUTATION is not even parsed (the variable is feature-gated).
cargo test --release --locked -p intent-server \
    --no-fail-fast --test attacks \
    -- --ignored
# The mutation proofs are their own target under the mutation-seams feature: they need
# the seams compiled in (they set INTENT_MUTATION per boot). Without the feature the
# proofs go red — which is the point: a shipped server cannot be tricked into
# carrying them.
cargo test --release --locked -p intent-server --features mutation-seams \
    --no-fail-fast --test mutation \
    -- --ignored
